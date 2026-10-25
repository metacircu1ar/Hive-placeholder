//! Bounded catalog discovery outside the database writer, with native ownership.
pub(crate) use super::binary::BinaryIdentity;
use super::{
    protocol::Model,
    registry::{Eviction, EvictionPolicy, Registry},
};
use crate::runtime::ModelDiscovery;
use crate::runtime::models::models_selected;
use crate::state::OperationKind;
use crate::{
    config::Runtime,
    error::{Error, Result},
    launch::LauncherCache,
    runtime::{hub::Wake, locks::MutexExt},
    state::State,
};
use std::error::Error as StdError;
use std::time::Duration;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

// Bound retained credential/binary-specific catalog entries.
const CATALOG_CACHE_ENTRIES: usize = 64;
// Bound model metadata retained across all catalog cache entries.
const CATALOG_MODEL_LIMIT: usize = 2000;
// Bound serialized catalog memory independently of model count.
const CATALOG_CACHE_BYTES: usize = 4 * 1024 * 1024;
// Limit simultaneous owned discovery subprocesses.
const MAX_DISCOVERY_JOBS: usize = 8;

struct Entry {
    generation: uuid::Uuid,
    key: Option<crate::state::KeyAuthority>,
    binary: BinaryIdentity,
    models: Vec<Model>,
    bytes: usize,
}
#[derive(serde::Deserialize)]
struct Catalog {
    data: Vec<Model>,
}
pub(crate) struct Catalogs {
    entries: Mutex<VecDeque<Entry>>,
    registry: Mutex<Weak<Mutex<Registry>>>,
    clock: Mutex<Arc<AtomicU64>>,
    settings: Runtime,
    shutdown: Arc<AtomicBool>,
    uncertain: Arc<AtomicBool>,
    wake: Arc<Wake>,
    jobs: Mutex<Jobs>,
    jobs_done: tokio::sync::Notify,
    job_slots: Arc<tokio::sync::Semaphore>,
}
#[derive(Default)]
struct Jobs {
    closing: bool,
    active: usize,
}
struct JobOwner(Arc<Catalogs>);
impl Drop for JobOwner {
    fn drop(&mut self) {
        self.0.jobs.lock_or_panic().active -= 1;
        self.0.jobs_done.notify_waiters();
    }
}
pub(crate) struct BindingInput {
    pub auth: String,
    pub profile: crate::config::Profile,
    pub generation: uuid::Uuid,
    pub state: State,
    pub binary: PathBuf,
    pub daemon_lock: Arc<crate::state::Lock>,
    pub launcher: Arc<LauncherCache>,
    pub credential: Option<crate::auth::Credential>,
    pub keys: Arc<crate::auth::Keys>,
}
impl Catalogs {
    pub(crate) fn new(
        settings: Runtime,
        shutdown: Arc<AtomicBool>,
        uncertain: Arc<AtomicBool>,
        wake: Arc<Wake>,
    ) -> Self {
        Self {
            entries: Mutex::default(),
            registry: Mutex::default(),
            clock: Mutex::default(),
            settings,
            shutdown,
            uncertain,
            wake,
            jobs: Mutex::default(),
            jobs_done: tokio::sync::Notify::new(),
            job_slots: Arc::new(tokio::sync::Semaphore::new(MAX_DISCOVERY_JOBS)),
        }
    }
    pub(super) fn attach(&self, registry: Weak<Mutex<Registry>>, clock: Arc<AtomicU64>) {
        let mut current = self.registry.lock_or_panic();
        assert!(
            current.upgrade().is_none(),
            "one provider pool owns catalog capacity"
        );
        *current = registry;
        *self.clock.lock_or_panic() = clock;
    }
    pub(crate) fn observe(
        &self,
        generation: uuid::Uuid,
        key: Option<crate::state::KeyAuthority>,
        binary: BinaryIdentity,
        models: Vec<Model>,
    ) {
        let bytes = models.iter().map(Model::retained_bytes).sum::<usize>();
        if models.is_empty() || models.len() > CATALOG_MODEL_LIMIT || bytes > CATALOG_CACHE_BYTES {
            return;
        }
        let mut entries = self.entries.lock_or_panic();
        entries.retain(|entry| {
            entry.generation != generation || entry.key != key || entry.binary != binary
        });
        entries.push_back(Entry {
            generation,
            key,
            binary,
            models,
            bytes,
        });
        while entries.len() > CATALOG_CACHE_ENTRIES
            || entries.iter().map(|entry| entry.bytes).sum::<usize>() > CATALOG_CACHE_BYTES
        {
            entries.pop_front();
        }
    }
    fn cached(
        &self,
        generation: uuid::Uuid,
        key: Option<&crate::state::KeyAuthority>,
        binary: &BinaryIdentity,
    ) -> Option<Vec<Model>> {
        let mut entries = self.entries.lock_or_panic();
        let index = entries.iter().position(|entry| {
            entry.generation == generation && entry.key.as_ref() == key && entry.binary == *binary
        })?;
        let entry = entries.remove(index)?;
        let models = entry.models.clone();
        entries.push_back(entry);
        Some(models)
    }
    pub(crate) fn invalidate(&self, generation: uuid::Uuid) {
        self.entries
            .lock_or_panic()
            .retain(|entry| entry.generation != generation);
    }
    pub(crate) async fn lookup_binding(
        self: &Arc<Self>,
        input: BindingInput,
    ) -> Result<(Vec<Model>, crate::auth::Credential)> {
        self.owned_work(move |owner| owner.resolve(input)).await
    }
    async fn owned_work<T: Send + 'static>(
        self: &Arc<Self>,
        work: impl FnOnce(&Self) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let permit = self
            .job_slots
            .clone()
            .try_acquire_owned()
            .map_err(|cause| {
                Error::agent_busy("Catalog discovery capacity is occupied; retry.")
                    .with_source(cause)
            })?;
        {
            let mut jobs = self.jobs.lock_or_panic();
            if jobs.closing {
                return Err(Error::daemon_stopping("Catalog discovery is stopping."));
            }
            jobs.active += 1;
        }
        let owner = self.clone();
        let running = JobOwner(self.clone());
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _running = running;
            work(&owner)
        })
        .await
        .map_err(|cause| {
            Error::internal_error("Catalog discovery owner failed.").with_source(cause)
        })?
    }
    pub(crate) async fn stop(&self) {
        self.jobs.lock_or_panic().closing = true;
        self.job_slots.close();
        loop {
            let done = self.jobs_done.notified();
            tokio::pin!(done);
            done.as_mut().enable();
            if self.jobs.lock_or_panic().active == 0 {
                return;
            }
            done.await;
        }
    }
    fn resolve(&self, input: BindingInput) -> Result<(Vec<Model>, crate::auth::Credential)> {
        let BindingInput {
            auth,
            profile,
            generation,
            state,
            binary,
            daemon_lock,
            launcher,
            credential,
            keys,
        } = input;
        let binding = state.generation(&auth, &profile, generation)?;
        // Cache hits still verify current credential ownership; catalog data is not
        // permission to bypass generation or credential-namespace ownership checks.
        state.operation(&binding, OperationKind::Models)?.finish()?;
        let credential = credential.map_or_else(|| keys.resolve(&state, &binding), Ok)?;
        let key = credential.authority().cloned();
        let binary = binary.canonicalize().map_err(Error::state_io_from)?;
        crate::provider_support::observe(&launcher, &state, &binary, profile.provider)?;
        let identity = BinaryIdentity::read(&binary)?;
        if let Some(models) = self.cached(binding.id, key.as_ref(), &identity) {
            credential.verify_source(&state, &binding)?;
            return Ok((models, credential));
        }
        let mut reservation = self.reserve()?;
        let ModelDiscovery { result, confirmed } = models_selected(
            &launcher,
            &state,
            &binding,
            &binary,
            Some(&daemon_lock),
            &credential,
        );
        if !confirmed {
            let error = crate::runtime::models::recovery_error(result);
            tracing::error!(name: "catalog.cleanup_unconfirmed",
                 code = error.code(),
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "retain_capacity");
            self.wake.failed(&error);
            self.uncertain.store(true, Ordering::Release);
            self.wake.all();
            return Err(error);
        }
        reservation.confirmed = true;
        let value = result?;
        state.generation(&auth, &profile, generation)?;
        if BinaryIdentity::read(&binary)? != identity {
            return Err(Error::agent_busy(
                "Provider executable changed during discovery; retry.",
            ));
        }
        let models = serde_json::from_value::<Catalog>(value)
            .map_err(|cause| {
                Error::provider_protocol_error("Invalid native model catalog.").with_source(cause)
            })?
            .data;
        if models.is_empty()
            || models.len() > CATALOG_MODEL_LIMIT
            || models.iter().map(Model::retained_bytes).sum::<usize>() > CATALOG_CACHE_BYTES
        {
            return Err(Error::provider_protocol_error(
                "Invalid native model catalog size.",
            ));
        }
        credential.verify_source(&state, &binding)?;
        self.observe(binding.id, key, identity, models.clone());
        Ok((models, credential))
    }
    fn reserve(&self) -> Result<Reservation> {
        if self.shutdown.load(Ordering::Acquire) || self.uncertain.load(Ordering::Acquire) {
            return Err(Error::daemon_stopping(
                "Catalog discovery is unavailable while the runtime is stopping or fenced.",
            ));
        }
        let registry = self.registry.lock_or_panic().upgrade().ok_or_else(busy)?;
        {
            let mut current = registry.lock_or_panic();
            current.refresh();
            if let Err(room) = current.charge_probe(&self.settings) {
                // Discovery returns the same retriable capacity error until its
                // selected process finishes owned cleanup and releases capacity.
                if !room.process
                    && let Eviction::Victim(shard) = current.eviction_victim(
                        EvictionPolicy::ColdDiscovery,
                        Duration::from_secs(self.settings.cohort_min_seconds),
                    )
                {
                    super::registry::begin_capacity_drain(
                        shard,
                        EvictionPolicy::ColdDiscovery,
                        None,
                    );
                }
                return Err(busy());
            }
        }
        Ok(Reservation {
            registry,
            wake: self.wake.clone(),
            uncertain: self.uncertain.clone(),
            confirmed: false,
        })
    }
}
#[derive(Debug, thiserror::Error)]
#[error("Physical catalog capacity is occupied")]
struct CatalogCapacity;
pub(crate) fn capacity_exhausted(error: &Error) -> bool {
    StdError::source(error).is_some_and(<dyn StdError>::is::<CatalogCapacity>)
}
fn busy() -> Error {
    Error::agent_busy(
        "No safe provider slot is available for model discovery; retry after \
            capacity is released.",
    )
    .with_source(CatalogCapacity)
}
struct Reservation {
    registry: Arc<Mutex<Registry>>,
    wake: Arc<Wake>,
    uncertain: Arc<AtomicBool>,
    confirmed: bool,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if self.confirmed {
            self.registry.lock_or_panic().release_probe();
        } else {
            tracing::warn!(name: "catalog.reservation_abandoned",
                 confirmed = false,
                 decision = "retain_capacity");
            self.uncertain.store(true, Ordering::Release);
        }
        self.wake.all();
    }
}

#[cfg(test)]
mod failure_tests;
#[cfg(test)]
mod key_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_catalog_evicts_empty_processes_only_for_process_capacity() {
        use crate::runtime::{provider::shard::corpus_support, test_support::RuntimeFixture};
        let fixture = RuntimeFixture::new();
        let pool = fixture.pool();
        let corpus_support::ShardSetup {
            shard,
            queues: _queues,
        } = corpus_support::shard(&fixture);
        shard.occupancy().slots.insert("retired".into());
        shard.occupancy().idle_since = Some(std::time::Instant::now());
        {
            let mut registry = pool.registry.lock_or_panic();
            registry.shards.push(shard.clone());
            registry.probes = 1;
        }
        let mut catalogs = Catalogs::new(
            Runtime {
                max_providers: 3,
                max_starting: 1,
                ..Runtime::default()
            },
            Arc::default(),
            Arc::default(),
            Arc::default(),
        );
        catalogs.attach(Arc::downgrade(&pool.registry), pool.clock.clone());
        assert!(catalogs.reserve().is_err());
        assert!(!shard.draining());
        catalogs.settings.max_providers = 2;
        assert!(catalogs.reserve().is_err());
        assert!(shard.draining());
        assert_eq!(pool.registry.lock_or_panic().probes, 1);
        assert!(!shard.capacity_released());
        drop(catalogs);
        drop(pool);
        drop(shard);
        fixture.finish();
    }
    #[tokio::test]
    async fn cancelled_lookup_keeps_discovery_owned_until_shutdown_join() {
        let catalogs = Arc::new(Catalogs::new(
            Runtime::default(),
            Arc::default(),
            Arc::default(),
            Arc::default(),
        ));
        let (entered, started) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let copy = catalogs.clone();
        let caller = tokio::spawn(async move {
            copy.owned_work(move |_| {
                entered.send(()).unwrap();
                blocked.recv().unwrap();
                Ok(())
            })
            .await
        });
        started.await.unwrap();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        let copy = catalogs.clone();
        let mut stopping = tokio::spawn(async move { copy.stop().await });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut stopping)
                .await
                .is_err()
        );
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), stopping)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(catalogs.jobs.lock_or_panic().active, 0);
        assert!(catalogs.owned_work(|_| Ok(())).await.is_err());
    }
}
