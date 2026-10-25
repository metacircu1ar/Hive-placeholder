//! Runtime state ownership, operations and agent lifecycle transitions.
mod approvals;
mod control;
mod messaging;
mod spawn;
use crate::runtime::api::InputContext as Op;
use control::ControlOperation;
pub(super) use spawn::text_file;
mod batch;
mod lifetimes;
use super::hub::Caller;
use super::{
    api::{self, Request, RequestEnvelope, SpawnArgs},
    domain::{Agent, AgentState, Control, EventKind, MessageState, TaskOutcome},
    invalid_request,
    store::Store,
};
pub(super) use crate::runtime::preflight::request as preflight;
use crate::{
    config::{Config, Lifetime},
    error::{Error, Result},
    state::{Lock, State},
};
use preflight::Facts;
use serde_json::Value;
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

// Maximum text size accepted from one configured instruction file.
pub(super) const INSTRUCTION_FILE_BYTES: usize = crate::limits::CONTROL_INPUT_BYTES;
// Read one byte beyond the instruction-file cap to detect oversized files.
const INSTRUCTION_FILE_READ_BYTES: u64 = INSTRUCTION_FILE_BYTES as u64 + 1;
// Maximum combined role and context instructions stored with one agent.
pub(super) const INSTRUCTION_SNAPSHOT_BYTES: usize = 262_144;
const _: () = assert!(INSTRUCTION_FILE_BYTES <= INSTRUCTION_SNAPSHOT_BYTES);
#[derive(Clone)]
pub(super) struct SpawnContext {
    pub(super) cwd: PathBuf,
    pub(super) instructions: String,
}

/// Services and ownership acquired before the runtime starts accepting jobs.
pub(super) struct CoreServices {
    pub binary_sha256: String,
    pub verification: Option<super::read_api::VerificationContext>,
    pub store: Store,
    pub config: Config,
    pub config_dir: PathBuf,
    pub state: State,
    pub binaries: super::binaries::ResolvedBinaries,
    pub daemon_lock: Arc<Lock>,
    pub launcher: Arc<crate::launch::LauncherCache>,
    pub shutdown: Arc<AtomicBool>,
}

pub(crate) struct Core {
    binary_sha256: String,
    verification: Option<super::read_api::VerificationContext>,
    store: Store,
    config: Config,
    config_dir: PathBuf,
    state: State,
    binaries: super::binaries::ResolvedBinaries,
    // Every daemon service shares this ownership description until its work finishes.
    daemon_lock: Arc<Lock>,
    launcher: Arc<crate::launch::LauncherCache>,
    leases: Arc<super::preflight::Leases>,
    shutdown: Arc<AtomicBool>,
    admission_tick: usize,
    running: HashSet<String>,
}
pub(super) fn eligible(agent: &Agent) -> Result<()> {
    if agent.terminal() {
        return Err(Error::agent_completed(
            "Completed, cancelled or lifetime-expired agents cannot accept work.",
        ));
    }
    Ok(())
}
impl Core {
    pub(super) fn new(services: CoreServices) -> Self {
        Self {
            binary_sha256: services.binary_sha256,
            verification: services.verification,
            store: services.store,
            config: services.config,
            config_dir: services.config_dir,
            state: services.state,
            binaries: services.binaries,
            daemon_lock: services.daemon_lock,
            launcher: services.launcher,
            leases: Arc::default(),
            shutdown: services.shutdown,
            admission_tick: 0,
            running: HashSet::new(),
        }
    }
    pub(super) fn verification(&self) -> Option<&super::read_api::VerificationContext> {
        self.verification.as_ref()
    }
    pub(super) fn store(&self) -> &Store {
        &self.store
    }
    pub(super) fn config(&self) -> &Config {
        &self.config
    }
    pub(super) fn config_dir(&self) -> &PathBuf {
        &self.config_dir
    }
    pub(super) fn state(&self) -> &State {
        &self.state
    }
    pub(super) fn binaries(&self) -> &super::binaries::ResolvedBinaries {
        &self.binaries
    }
    pub(super) fn daemon_lock(&self) -> &Arc<Lock> {
        &self.daemon_lock
    }
    pub(super) fn binary_sha256(&self) -> &str {
        &self.binary_sha256
    }
    pub(super) fn launcher(&self) -> &Arc<crate::launch::LauncherCache> {
        &self.launcher
    }
    pub(super) fn leases(&self) -> &Arc<super::preflight::Leases> {
        &self.leases
    }
    pub(super) fn shutdown(&self) -> &Arc<AtomicBool> {
        &self.shutdown
    }
    pub(super) fn running(&self) -> &HashSet<String> {
        &self.running
    }
    pub(super) fn started(&mut self, id: String) {
        self.running.insert(id);
    }
    pub(super) fn stopped(&mut self, id: &str) {
        self.running.remove(id);
    }
    pub(super) fn admission_class(&mut self, service: &[usize]) -> usize {
        let class = service[self.admission_tick % service.len()];
        self.admission_tick = self.admission_tick.wrapping_add(1);
        class
    }
    #[cfg(test)]
    pub(super) fn config_mut(&mut self) -> &mut Config {
        &mut self.config
    }
    #[cfg(test)]
    pub(super) fn binaries_mut(&mut self) -> &mut super::binaries::ResolvedBinaries {
        &mut self.binaries
    }
    #[cfg(test)]
    pub(super) fn set_admission_tick(&mut self, tick: usize) {
        self.admission_tick = tick;
    }
    #[cfg(test)]
    pub(super) fn replace_store(&mut self, store: Store) {
        self.store = store;
    }
    #[cfg(test)]
    pub(super) fn clear_running(&mut self) {
        self.running.clear();
    }

    pub(super) fn quarantine_provider(&mut self, provider: &str) -> Result<()> {
        self.store.fence_provider(provider)
    }
    pub(super) fn atomic<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.store.begin()?;
        match f(self) {
            Ok(v) => {
                if let Err(error) = self.store.commit() {
                    self.store.rollback_after(&error);
                    return Err(error);
                }
                Ok(v)
            }
            Err(e) => {
                self.store.rollback_after(&e);
                Err(e)
            }
        }
    }
    pub(super) fn replay(
        &self,
        caller: &Caller,
        id: &str,
        args: &RequestEnvelope,
    ) -> Result<Option<Value>> {
        if caller.is_system() {
            args.request.caller_policy(caller)?;
        }
        self.store.prior(caller.name(), id, &args.fingerprint())
    }
    pub(super) fn dispatch(
        &mut self,
        caller: &Caller,
        id: &str,
        args: &RequestEnvelope,
        facts: &Facts,
    ) -> Result<Value> {
        if caller.is_system() {
            args.request.caller_policy(caller)?;
        }
        if args.request.is_preview() {
            return self.atomic(|core| core.operation(caller, id, args, facts));
        }
        if args.request.is_read_only() {
            return self.operation(caller, id, args, facts);
        }
        let body = args.fingerprint();
        // Deduplication precedes lifecycle checks: successful retries survive completion.
        if let Some(old) = self.store.prior(caller.name(), id, &body)? {
            return Ok(old);
        }
        self.atomic(|core| {
            if let Some(agent) = caller.agent_id() {
                eligible(&core.store.agent(agent)?)?;
            }
            let result = core.operation(caller, id, args, facts)?;
            core.store.remember(caller.name(), id, &body, &result)?;
            Ok(result)
        })
    }
    fn operation(
        &mut self,
        caller: &Caller,
        request_id: &str,
        envelope: &RequestEnvelope,
        facts: &Facts,
    ) -> Result<Value> {
        envelope.request.caller_policy(caller)?;
        check_writer_request(&envelope.request)?;
        if envelope.request.is_read_only() {
            return super::read_api::dispatch(
                &self.store,
                &envelope.request,
                self.verification.as_ref(),
                &self.config,
            );
        }
        match &envelope.request {
            #[cfg(feature = "comb")]
            Request::Evidence(args) => self.store.submit_evidence(caller, args).map(api::value),
            #[cfg(feature = "comb")]
            Request::CollectorAdmin(args) => {
                self.store.collector_admin(args)?;
                Ok(api::value(api::StatusResponse { status: "updated" }))
            }
            Request::Spawn(args) => self.spawn(caller, args, envelope, facts),
            Request::Batch(args) => self.batch(caller, request_id, args, envelope, facts),
            Request::Send(args) => self.send(caller, args, facts.attachments(envelope)?),
            Request::Ask(args) => self.ask(caller, args),
            Request::Respond(args) => self.respond(args),
            Request::SetOptions(args) => self.set_options(caller, args, facts),
            Request::SetAuth(args) => self.set_auth(args, facts),
            Request::SetName(args) => self.set_name(args),
            Request::LoginRestored(args) => self.login_restored(args, facts),
            Request::Complete(args) => self.complete(caller, args),
            Request::Terminate(args) => self.terminate(caller, args),
            Request::Purge(args) => {
                let api::PurgeOptions { recursive, dry_run } = args.options()?;
                self.store
                    .purge(
                        args.target.required(Op::Purge, "/target")?,
                        recursive,
                        dry_run,
                        &self.running,
                    )
                    .map(api::value)
            }
            Request::Gc(args) => self.store.gc(args, &self.running).map(api::value),
            Request::Stop(args) => self.control(caller, args, ControlOperation::Stop, facts),
            Request::Resume(args) => self.control(caller, args, ControlOperation::Resume, facts),
            Request::Interrupt(args) => {
                self.control(caller, args, ControlOperation::Interrupt, facts)
            }
            Request::Shutdown => {
                tracing::info!(name: "daemon.shutdown_requested", decision = "shutdown");
                self.shutdown.store(true, Ordering::Relaxed);
                Ok(api::value(api::StatusResponse { status: "stopping" }))
            }
            Request::Unsupported => Err(Error::unsupported_operation(
                "Operation is not implemented in this runtime increment.",
            )),
            _ => Err(invalid_request()),
        }
    }
}

fn check_writer_request(request: &Request) -> Result<()> {
    if matches!(request, Request::Agents(args) if args.stalled_requested()) {
        return Err(Error::unsupported_operation(
            "Stalled group queries require the reader service.",
        ));
    }
    if matches!(
        request,
        Request::Live(_) | Request::Tree(_) | Request::Attach(_)
    ) {
        return Err(Error::unsupported_operation(
            "Live viewing requests are direct control requests and cannot be batched.",
        ));
    }
    if matches!(request, Request::Usage(_)) {
        return Err(Error::unsupported_operation(
            "Usage reports are direct control requests and cannot be batched.",
        ));
    }
    Ok(())
}
