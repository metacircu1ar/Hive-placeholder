//! Shared provider pool entry points and runtime resource reporting.
use super::{
    acquire::Acquisition,
    error::unavailable,
    registry::{CapacityPressure, RecoveredReservation, RecoveryCapacity, Registry},
    session::Session,
    shard::{Cmd, Route, Shard},
};
use crate::{
    config::Runtime,
    error::Result,
    runtime::{
        api,
        hub::{Hub, Wake},
        locks::MutexExt,
    },
    state::{Lock, RuntimeLease},
};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use tokio::sync::{Notify, mpsc, oneshot, watch};

// Maximum frames waiting for one agent to consume them.
const SESSION_QUEUE_CAPACITY: usize = 256;

#[cfg(test)]
mod tests;

struct PendingAttach<'a> {
    pool: &'a Pool,
    shard: &'a Shard,
    agent: &'a str,
    armed: bool,
    cancelled: Arc<AtomicBool>,
}
impl Drop for PendingAttach<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.cancelled.store(true, Ordering::Release);
        {
            let _registry = self.pool.registry.lock_or_panic();
            let mut occupancy = self.shard.occupancy();
            occupancy.users.remove(self.agent);
            occupancy.activity.remove(self.agent);
            occupancy.idle_since = None;
            // Keep the slot charged. Retire an empty abandoned process, while
            // healthy peers keep using a process that still has owners.
            if occupancy.users.is_empty() {
                self.shard.abandon_attach();
            }
        }
        self.shard.wake_owner();
        self.pool.changed.notify_waiters();
        self.pool.wake.all();
    }
}

#[derive(Clone)]
pub(crate) struct Pool {
    pub(crate) turns: Arc<tokio::sync::Semaphore>,
    pub(super) registry: Arc<Mutex<Registry>>,
    pub(super) changed: Arc<Notify>,
    pub(super) settings: Runtime,
    pub(super) hub: Hub,
    pub(super) wake: Arc<Wake>,
    pub(super) uncertain: Arc<AtomicBool>,
    pub(super) shutdown: Arc<AtomicBool>,
    pub(super) clock: Arc<AtomicU64>,
    /// The newest quota usage each login reported, for `/usage`.
    pub(crate) usage: Arc<super::usage::Usages>,
}
pub(crate) struct SessionRequest<'a> {
    pub agent: &'a str,
    pub previous_provider: Option<&'a str>,
    pub auth: &'a str,
    pub command: super::Launch,
    pub lease: Arc<RuntimeLease>,
    pub daemon_lock: Arc<Lock>,
    pub class: usize,
}
impl Pool {
    pub(super) fn publish_exit(&self, shard: &Shard) {
        shard.mark_exited();
        shard.done.notify_waiters();
        self.changed.notify_waiters();
        self.wake.all();
    }

    pub(crate) fn new(
        settings: Runtime,
        hub: Hub,
        uncertain: Arc<AtomicBool>,
        shutdown: Arc<AtomicBool>,
        recovered: Vec<RecoveredReservation>,
    ) -> Self {
        let registry = Arc::new(Mutex::new(Registry {
            probes: 0,
            unresolved_auth: HashSet::new(),
            shards: Vec::new(),
            recovered,
        }));
        let clock = Arc::new(AtomicU64::new(1));
        hub.catalogs
            .attach(Arc::downgrade(&registry), clock.clone());
        Self {
            turns: Arc::new(tokio::sync::Semaphore::new(settings.max_active_turns)),
            registry,
            changed: Arc::new(Notify::new()),
            settings,
            wake: hub.wake.clone(),
            hub,
            uncertain,
            shutdown,
            clock,
            usage: Arc::default(),
        }
    }
    pub(crate) async fn acquire(&self, request: SessionRequest<'_>) -> Result<Session> {
        let SessionRequest {
            agent,
            previous_provider,
            auth,
            command,
            lease,
            daemon_lock,
            class,
        } = request;
        let acquisition = Acquisition {
            pool: self,
            agent,
            previous_provider,
            auth,
            command: &command,
            lease: &lease,
            daemon_lock,
            class,
        };
        let shard = acquisition.wait().await?;
        self.attach(shard, agent).await
    }
    async fn attach(&self, shard: Arc<Shard>, agent: &str) -> Result<Session> {
        let (tx, rx) = mpsc::channel(SESSION_QUEUE_CAPACITY);
        let (failure, failed) = watch::channel(None);
        let (ack, ready) = oneshot::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        // Drop this guard before the receivers, fencing a cancelled route before
        // any concurrent send could mistake its closed receiver for owner failure.
        let mut pending = PendingAttach {
            pool: self,
            shard: &shard,
            agent,
            armed: true,
            cancelled: cancelled.clone(),
        };
        let attached = attach_route(
            &shard,
            agent,
            Route {
                attach_cancelled: cancelled,
                tx,
                bytes: Arc::new(AtomicUsize::new(0)),
                failure,
            },
            ack,
            ready,
        )
        .await;
        pending.armed = attached.is_err();
        drop(pending);
        match attached {
            Ok(started) => Ok(Session {
                pool: self.clone(),
                shard,
                agent: agent.into(),
                events: rx,
                failure: failed,
                started,
                thread: None,
                released: false,
            }),
            Err(e) => {
                if shard.commands_closed() {
                    // A bootstrap failure may fence Attach before its cleanup
                    // receipt is written. Preserve the final failure semantics.
                    shard.wait_exited().await;
                }
                Err(shard
                    .catalog
                    .get()
                    .and_then(|catalog_result| catalog_result.as_ref().err())
                    .cloned()
                    .unwrap_or(e))
            }
        }
    }
    pub(crate) fn drain(&self) {
        self.drain_one(&self.registry.lock_or_panic(), CapacityPressure::Actors);
    }
    pub(crate) fn drain_catalog(&self) {
        self.drain_one(&self.registry.lock_or_panic(), CapacityPressure::Processes);
    }
    pub(crate) fn snapshot(&self) -> Value {
        let mut registry = self.registry.lock_or_panic();
        registry.refresh();
        let RecoveryCapacity {
            processes: recovered_processes,
            slots: recovered_slots,
        } = registry.recovered_capacity();
        let resources = api::RuntimeResources {
            provider_versions: registry
                .shards
                .iter()
                .filter_map(|shard| shard.compatibility.as_ref().map(|e| &e.observation))
                .collect(),
            active_turns: self
                .settings
                .max_active_turns
                .saturating_sub(self.turns.available_permits()),
            reserved_shards: recovered_processes + registry.probes,
            recovered_provider_reservations: recovered_processes,
            unreaped_provider_reservations: 0,
            recovered_unresolved_activity_profiles: registry.unresolved_auth.len(),
            resident_session_slots: recovered_slots,
            attached_agents: 0,
            starting_shards: registry.probes,
            warm_shards: 0,
            lingering_shards: 0,
            draining_shards: 0,
            limits: &self.settings,
        };
        api::value(
            registry
                .shards
                .iter()
                .filter(|shard| !shard.capacity_released())
                .fold(resources, |mut resources, shard| {
                    let occupancy = shard.occupancy();
                    let starting = shard.starting();
                    let draining = shard.draining();
                    resources.reserved_shards += 1;
                    resources.unreaped_provider_reservations += usize::from(shard.exited());
                    resources.resident_session_slots += occupancy.slots.len();
                    resources.attached_agents += occupancy.users.len();
                    resources.starting_shards += usize::from(starting);
                    resources.warm_shards +=
                        usize::from(occupancy.slots.is_empty() && !starting && !draining);
                    resources.lingering_shards +=
                        usize::from(occupancy.idle_since.is_some() && !draining);
                    resources.draining_shards += usize::from(draining);
                    resources
                }),
        )
    }

    pub(crate) async fn shutdown(&self) {
        self.wake.all();
        let shards = self.registry.lock_or_panic().shards.clone();
        // Stop, wake and observe each owner before advancing shutdown.
        for shard in shards {
            shard.request_stop();
            shard.wake_owner();
            shard.wait_exited().await;
        }
    }
}

async fn attach_route(
    shard: &Shard,
    agent: &str,
    route: Route,
    ack: oneshot::Sender<Result<super::Started>>,
    ready: oneshot::Receiver<Result<super::Started>>,
) -> Result<super::Started> {
    shard
        .command(Cmd::Attach(agent.into(), route, ack), false)
        .await?;
    ready
        .await
        .map_err(|cause| unavailable().with_source(cause))?
}
