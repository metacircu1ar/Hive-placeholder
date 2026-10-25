//! Prepare private daemon state and ownership before starting background services.
use super::{
    Arc, AtomicBool, BoundControlSocket, Config, Core, Error, Lock, Path, PathBuf, Queries, Result,
    State, UnixListener, bind_control_socket, check_daemon_owner, invalid_request, provider,
    recover_store, recovery_capacity, state, write_daemon_owner,
};
use crate::runtime::core::CoreServices;
#[cfg(test)]
use std::fs;

struct Configuration {
    admission: hive_setup_support::admission::Shared,
    config_path: PathBuf,
    config: Config,
    root: PathBuf,
    runtime: PathBuf,
    binaries: super::super::binaries::ResolvedBinaries,
    logging: hive_log::Config,
}
impl Configuration {
    fn load(config_path: PathBuf, root: PathBuf, binaries: super::super::Binaries) -> Result<Self> {
        let config_path = config_path.canonicalize().map_err(Error::state_io_from)?;
        let (config, logging) = super::daemon_configuration(&config_path)?;
        let admission = State::new(root.clone())?.admission()?;
        State::new(root.clone())?.check_format()?;
        // Cache either path or error before starting the single database writer.
        // A missing unused provider is harmless and never blocks daemon startup.
        let binaries = binaries.resolved();
        let _initialization = State::new(root.clone())?.initialize()?;
        let runtime = root.join("runtime");
        state::directory(&runtime)?;
        Ok(Self {
            admission,
            config_path,
            config,
            root,
            runtime,
            binaries,
            logging,
        })
    }
}
struct Ownership {
    daemon_lock: Arc<Lock>,
    marker: PathBuf,
    boot: String,
}

pub(super) struct OwnedDaemon {
    configuration: Configuration,
    ownership: Ownership,
}
impl OwnedDaemon {
    pub fn open(
        config_path: PathBuf,
        root: PathBuf,
        binaries: super::super::Binaries,
    ) -> Result<Self> {
        let configuration = Configuration::load(config_path, root, binaries)?;
        let ownership =
            Ownership::acquire(&configuration.runtime, configuration.admission.clone())?;
        State::new(configuration.root.clone())?.cleanup_preparations(&ownership.daemon_lock)?;
        // Before the logger or Tokio starts threads; SQLite companions stay private.
        crate::os::private_umask();
        Ok(Self {
            configuration,
            ownership,
        })
    }
    pub fn runtime(&self) -> &Path {
        &self.configuration.runtime
    }
    pub fn lock(&self) -> Arc<Lock> {
        self.ownership.daemon_lock.clone()
    }
    pub fn logging(&self) -> hive_log::Config {
        self.configuration.logging.clone()
    }
}
impl Ownership {
    fn acquire(runtime: &Path, admission: hive_setup_support::admission::Shared) -> Result<Self> {
        // The outer owner outlives workers, provider threads, and the hub writer.
        // Sharing ownership never duplicates the inherited file description.
        let daemon_lock = Arc::new(
            Lock::acquire(&runtime.join("daemon.lock"))
                .map_err(|cause| {
                    Error::daemon_busy("A daemon owns this state directory.").with_source(cause)
                })?
                .with_admission(admission),
        );
        let marker = runtime.join("owner.json");
        let boot = state::boot_identity()?;
        check_daemon_owner(&marker, &daemon_lock)?;
        Ok(Self {
            daemon_lock,
            marker,
            boot,
        })
    }
}

pub(super) struct PreparedDaemon {
    pub runtime: PathBuf,
    pub daemon_lock: Arc<Lock>,
    pub marker: PathBuf,
    pub queries: Queries,
    pub socket: PathBuf,
    pub listener: UnixListener,
    pub shutdown: Arc<AtomicBool>,
    pub cleanup_uncertain: Arc<AtomicBool>,
    pub settings: crate::config::Runtime,
    pub core: Core,
    pub recovered: Vec<provider::RecoveredReservation>,
}
impl PreparedDaemon {
    pub fn open(owned: OwnedDaemon) -> Result<Self> {
        let Configuration {
            admission: _,
            config_path,
            config,
            root,
            runtime,
            binaries,
            logging: _,
        } = owned.configuration;
        let Ownership {
            daemon_lock,
            marker,
            boot,
        } = owned.ownership;
        tracing::info!(name: "daemon.binaries_resolved", phase = "cached_results");
        #[cfg(feature = "barrier-delay")]
        crate::observability::state_sync::barrier_delay::configure()?;
        crate::config::deprecations::warn(&config.deprecations);
        let store = recover_store(&runtime)?;
        #[cfg(feature = "comb")]
        configure_collectors(&store, &config)?;
        let queries = Queries::open_read_only(&runtime.join("runtime.sqlite"))?;
        #[cfg(not(test))]
        let launcher = crate::launch::system_cache();
        #[cfg(test)]
        let launcher = crate::launch::test_cache()?;
        launcher.initialize()?;
        let BoundControlSocket { socket, listener } = bind_control_socket(&root)?;
        tracing::info!(name: "daemon.socket_bound", success = true);
        let binary_sha256 = launcher.digest()?;
        write_daemon_owner(&marker, &boot, &daemon_lock, &binary_sha256)?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let cleanup_uncertain = Arc::new(AtomicBool::new(false));
        let settings = config.runtime.clone();
        let mut core = Core::new(CoreServices {
            binary_sha256,
            verification: verification_context(&binaries, &config_path, &root),
            store,
            config,
            config_dir: config_path.parent().ok_or_else(invalid_request)?.into(),
            state: State::new(root)?.for_runtime(daemon_lock.clone())?,
            binaries,
            daemon_lock: daemon_lock.clone(),
            launcher,
            shutdown: shutdown.clone(),
        });
        core.recover_auth_cutoffs()?;
        tracing::info!(name: "recovery.auth_cutoffs", success = true);
        let recovered = recovery_capacity(&core)?;
        tracing::info!(name: "recovery.reservations", count = recovered.len());
        core.store().changes();
        #[cfg(test)]
        super::failure_tests::install_fault(core.store());
        Ok(Self {
            runtime,
            daemon_lock,
            marker,
            queries,
            socket,
            listener,
            shutdown,
            cleanup_uncertain,
            settings,
            core,
            recovered,
        })
    }
}

#[cfg(feature = "comb")]
fn configure_collectors(store: &super::Store, config: &Config) -> Result<()> {
    // Unreadable identities stay paused until an explicit human rebind.
    let identities = config
        .comb_collectors
        .iter()
        .filter_map(|(name, collector)| {
            comb_protocol::service::auth::Connection::read(&collector.credential_file)
                .inspect_err(|error| {
                    tracing::warn!(name: "recovery.collector_paused",
                     error = error as &(dyn std::error::Error + 'static),
                     decision = "await_explicit_rebind");
                })
                .ok()
                .map(|connection| (name.clone(), connection.credential.comb_id.to_string()))
        })
        .collect();
    store.configure_collectors(&config.comb_collectors, &identities)
}

#[cfg(test)]
mod tests;

fn verification_context(
    binaries: &crate::runtime::binaries::ResolvedBinaries,
    config_path: &Path,
    root: &Path,
) -> Option<crate::runtime::read_api::VerificationContext> {
    binaries
        .resolve(crate::config::Provider::Claude)
        .inspect_err(|error| {
            tracing::debug!(name: "recovery.verification_unavailable",
                             provider = "claude",
                             error = error as &(dyn std::error::Error + 'static),
                             decision = "disable_verification");
        })
        .ok()
        .and_then(|binary| {
            super::super::read_api::VerificationContext::new(config_path, root, &binary)
                .inspect_err(|error| {
                    tracing::warn!(name: "recovery.verification_unavailable",
                                     provider = "claude",
                                     error = error as &(dyn std::error::Error + 'static),
                                     decision = "disable_verification");
                })
                .ok()
        })
}
