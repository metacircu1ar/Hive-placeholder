//! Create shard queues, authorize native startup, and publish the provider catalog.
use super::init::{ShardChannels, ShardIdentity, channels};
use super::{
    Arc, Command, ESSENTIAL_QUEUE_CAPACITY, Error, Lock, MODEL_PAGE_BUDGET, Model, MutexExt,
    NORMAL_QUEUE_CAPACITY, Ordering, PRIORITY_QUEUE_CAPACITY, Pool, ProviderEventKind, Registry,
    Result, Rpc, RuntimeLease, Shard, ShardLoop, Shutdown, Started, Uuid, Value, protocol,
};
use crate::launch::LauncherCache;
use crate::runtime::api::{self, events};
use crate::runtime::branching::BranchFailure;
use crate::runtime::branching::BranchPhase;
use crate::runtime::domain::ProviderProcess;
use crate::runtime::provider::Backend;
use crate::runtime::provider::Launch;
use crate::runtime::provider::LaunchOwner;
use crate::runtime::provider::ShutdownMethod;
use crate::state;
use crate::state::ProcessIdentity;
use crate::state::ProviderEvidence;
use std::panic;
use std::thread;

impl Pool {
    pub(crate) fn start_shard(
        &self,
        registry: &mut Registry,
        auth: &str,
        launch: &Launch,
        lease: Arc<RuntimeLease>,
        daemon_lock: Arc<Lock>,
    ) -> Result<Arc<Shard>> {
        let (shard, channels) = self.reserve_shard(auth, launch, &lease)?;
        registry.shards.push(shard.clone());
        let thread_shard = shard.clone();
        let launch_owner = launch.owner.clone();
        let launcher = launch.launcher.clone();
        let pool = self.clone();
        let owned = copy_command(&launch.command);
        let credential = launch.credential.clone();
        // Keep reservation publication and thread creation under the registry
        // fence: callers cannot observe a reserved shard with no servicing thread.
        // Only the servicing thread is created here; native provider launch runs
        // on that thread. We never join or wait for it while holding registry,
        // and that thread acquires registry only after this owner can release it.
        if let Err(error) = thread::Builder::new()
            .name("hive-provider".into())
            .spawn(move || {
                let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                    run_shard(
                        &pool,
                        &thread_shard,
                        ThreadInputs {
                            credential,
                            command: owned,
                            launcher,
                            launch_owner,
                            lease,
                            daemon_lock,
                            channels,
                        },
                    );
                }));
                if result.is_err() {
                    let error = Error::internal_error("Provider owner panicked.");
                    record_start_failure(&thread_shard, "panic", &error);
                    quarantine_after_failure(&pool, &thread_shard, &error);
                    pool.publish_exit(&thread_shard);
                }
                pool.retire_record(&thread_shard);
            })
        {
            shard.release_capacity();
            shard.mark_exited();
            return Err(Error::state_io_from(error));
        }
        Ok(shard)
    }
    fn reserve_shard(
        &self,
        auth: &str,
        launch: &Launch,
        lease: &Arc<RuntimeLease>,
    ) -> Result<(Arc<Shard>, ShardChannels)> {
        let (inputs, channels) = channels([
            NORMAL_QUEUE_CAPACITY,
            PRIORITY_QUEUE_CAPACITY,
            ESSENTIAL_QUEUE_CAPACITY,
        ])?;
        let shard = Arc::new(Shard::new(
            ShardIdentity {
                id: Uuid::new_v4().to_string(),
                auth,
                process_options: &launch.process_options,
                key: launch.credential.authority(),
                backend: &launch.backend,
                compatibility: launch.compatibility.as_ref(),
                lease,
            },
            inputs,
            true,
            || self.clock.fetch_add(1, Ordering::Relaxed),
        ));
        Ok((shard, channels))
    }
}

struct ThreadInputs {
    credential: crate::auth::Credential,
    command: Command,
    launcher: Arc<LauncherCache>,
    launch_owner: Option<crate::runtime::provider::LaunchOwner>,
    lease: Arc<RuntimeLease>,
    daemon_lock: Arc<Lock>,
    channels: ShardChannels,
}
struct ShardThread<'a> {
    pool: &'a Pool,
    launcher: &'a LauncherCache,
    shard: &'a Shard,
    lease: &'a RuntimeLease,
    owner: Option<&'a crate::runtime::provider::LaunchOwner>,
}
fn copy_command(command: &Command) -> Command {
    // The command factory uses an allowlisted, cleared environment. Preserve
    // its exact executable, arguments, environment and working directory.
    let mut owned = Command::new(command.get_program());
    owned.args(command.get_args()).env_clear();
    for (key, value) in command.get_envs() {
        if let Some(value) = value {
            owned.env(key, value);
        }
    }
    if let Some(cwd) = command.get_current_dir() {
        owned.current_dir(cwd);
    }
    owned
}

pub(super) fn validate_account(rpc: &mut Rpc) -> Result<()> {
    rpc.validate_account()
}
pub(super) fn bootstrap(
    rpc: &mut Rpc,
    credential: &crate::auth::Credential,
    home: &std::path::Path,
) -> Result<Vec<Model>> {
    rpc.initialize_authenticated(credential, home)?;
    let mut catalog = Vec::new();
    let mut cursor = Value::Null;
    let mut lookup = crate::runtime::provider::ModelLookup::default();
    for _ in 0..MODEL_PAGE_BUDGET {
        let value = lookup.page(rpc, &cursor)?;
        catalog.extend(value.catalog().ok_or_else(protocol)?);
        cursor = value.cursor().clone();
        if cursor.is_null() {
            return Ok(catalog);
        }
    }
    Err(Error::output_limit(
        "Model catalog exceeded the pagination limit.",
    ))
}
// The lease, evidence, channel receivers and Rpc share this scope through driver
// shutdown; phase helpers borrow their owners instead of shortening their lifetime.
fn run_shard(pool: &Pool, shard: &Arc<Shard>, inputs: ThreadInputs) {
    let ThreadInputs {
        credential,
        command,
        launcher,
        launch_owner,
        lease,
        daemon_lock,
        mut channels,
    } = inputs;
    let thread = ShardThread {
        pool,
        launcher: &launcher,
        shard,
        lease: &lease,
        owner: launch_owner.as_ref(),
    };
    shard.observe_binary(&command);
    // Bind the Result so a failed reservation stays alive through exit publication.
    let reservation = lease.begin_provider(&shard.id);
    let mut evidence = match reservation {
        Ok(evidence) => evidence,
        Err(error) => {
            record_start_failure(shard, "reservation", &error);
            drop(daemon_lock);
            pool.quarantine(shard);
            shard.release_capacity();
            pool.publish_exit(shard);
            return;
        }
    };
    let before = state::now();
    let Some(mut rpc) = thread.start(command, daemon_lock, &mut evidence) else {
        return;
    };
    let started = Started {
        pid: rpc.pid(),
        before,
        after: state::now(),
    };
    let recorded = thread.record_started(&rpc, &started);
    if let Err(error) = &recorded {
        pool.hub.stop_after_failure(error);
        record_start_failure(shard, "record_started", error);
    }
    let mut driver = ShardLoop::new(pool, shard, &mut rpc, &started, &mut channels);
    let result = driver.run(recorded, &evidence, &credential);
    driver.shutdown(&mut evidence, result);
}

impl ShardThread<'_> {
    fn record_started(&self, rpc: &Rpc, started: &Started) -> Result<()> {
        let Self { pool, shard, .. } = *self;
        *shard.process.lock_or_panic() = ProcessIdentity::read(rpc.pid())
            .inspect_err(|error| {
                tracing::warn!(name: "provider.identity_unavailable",
                     provider_id = shard.id.as_str(),
                     pid = rpc.pid(),
                     error = error as &(dyn std::error::Error + 'static),
                     decision = "retain_receipt_ownership");
            })
            .ok()
            .flatten();
        let generation = shard.id.clone();
        let info = started.clone();
        pool.hub.blocking_with(move |core| {
            core.store().provider_event(
                &generation,
                ProviderEventKind::Started,
                api::value(events::ProcessStart {
                    pid: info.pid,
                    spawn_before: info.before,
                    spawn_after: info.after,
                }),
            )
        })
    }
    fn start(
        &self,
        command: Command,
        daemon_lock: Arc<Lock>,
        evidence: &mut ProviderEvidence,
    ) -> Option<Rpc> {
        let Self {
            pool, shard, lease, ..
        } = *self;
        // Both owners remain in this thread throughout spawn; the outer daemon lock
        // also survives pool shutdown and the hub writer's join in serve_async.
        let started_rpc = Rpc::start_authorized(
            self.launcher,
            command,
            shard.backend.clone(),
            vec![Some(lease.fd())],
            |pid| {
                if let Some(compatibility) = &shard.compatibility {
                    compatibility.check()?;
                }
                bind_gated_conversation(pool, shard, lease, evidence, self.owner)?;
                evidence.started(pid)
            },
        );
        // Only spawn needs this extra owner. Release it before any exit notification,
        // including spawn failure; the original outer owner controls final release.
        drop(daemon_lock);
        match started_rpc {
            Ok(r) => Some(r),
            Err(error) if matches!(error, Error::SpawnFailed { .. }) => {
                // unstarted may attach proven-unsent retry eligibility before publication.
                tracing::error!(name: "provider.start_failed",
                     provider_id = shard.id.as_str(),
                     phase = "spawn",
                     code = error.code(),
                     error = &error as &(dyn std::error::Error + 'static),
                     decision = "settle_unstarted");
                self.unstarted(evidence, error);
                None
            }
            Err(error) => {
                record_start_failure(shard, "native_start", &error);
                quarantine_after_failure(pool, shard, &error);
                pool.publish_exit(shard);
                None
            }
        }
    }
    fn unstarted(&self, evidence: &mut ProviderEvidence, error: Error) {
        let Self { pool, shard, .. } = *self;
        // No native child existed. Resolve its reservation without blocking
        // other conversations authenticated through the same profile.
        let finished = evidence.finish(true, true);
        if let Err(secondary) = &finished {
            tracing::error!(name: "provider.unstarted_receipt_failed",
                 provider_id = shard.id.as_str(),
                 code = secondary.code(),
                 primary = &error as &(dyn std::error::Error + 'static),
                 secondary = secondary as &(dyn std::error::Error + 'static),
                 decision = "quarantine");
        }
        let clean = finished.is_ok();
        let safe_retry = if clean {
            match settle_unstarted_branch(pool, shard, self.owner) {
                Ok(retry) => retry,
                Err(secondary) => {
                    pool.hub.stop_after_failure(&secondary);
                    tracing::error!(name: "provider.branch_settlement_failed",
                         provider_id = shard.id.as_str(),
                         code = secondary.code(),
                         primary = &error as &(dyn std::error::Error + 'static),
                         secondary = &secondary as &(dyn std::error::Error + 'static),
                         decision = "shutdown_retain_evidence");
                    false
                }
            }
        } else {
            false
        };
        let _ = shard.catalog.set(Err(if safe_retry {
            error.before_submission()
        } else {
            error
        }));
        *shard.report.lock_or_panic() = Some(Shutdown {
            method: ShutdownMethod::NotStarted,
            confirmed: clean,
            quiescent: true,
            reaped: false,
            exit_code: None,
        });
        shard.release_capacity();
        if !clean {
            pool.quarantine(shard);
        }
        pool.publish_exit(shard);
    }
}

fn record_start_failure(shard: &Shard, phase: &'static str, error: &Error) {
    tracing::error!(name: "provider.start_failed",
         provider_id = shard.id.as_str(),
         phase = phase,
         code = error.code(),
         error = error as &(dyn std::error::Error + 'static),
         decision = "quarantine");
    // Bootstrap may already have published its result; never replace that observation.
    let _ = shard.catalog.set(Err(error.clone()));
}
fn quarantine_after_failure(pool: &Pool, shard: &Shard, primary: &Error) {
    pool.quarantine(shard);
    let failed_provider = shard.id.clone();
    if let Err(secondary) = pool
        .hub
        .blocking_with(move |core| core.quarantine_provider(&failed_provider))
    {
        pool.hub.stop_after_failure(&secondary);
        tracing::error!(name: "provider.quarantine_record_failed",
             provider_id = shard.id.as_str(),
             code = secondary.code(),
             primary = primary as &(dyn std::error::Error + 'static),
             secondary = &secondary as &(dyn std::error::Error + 'static),
             sqlite_code = crate::runtime::store::sqlite_code(&secondary),
             decision = "shutdown_retain_evidence");
    }
}

fn bind_gated_conversation(
    pool: &Pool,
    shard: &Shard,
    lease: &RuntimeLease,
    evidence: &ProviderEvidence,
    launch_owner: Option<&LaunchOwner>,
) -> Result<()> {
    // Claude opens/resumes at exec, unlike Codex's later thread RPC.
    // Bind the exact ownership attempt while native exec is still gated.
    if let (Backend::Claude(config), Some(LaunchOwner { agent, execution })) =
        (&shard.backend, launch_owner)
    {
        evidence.bind_conversation(agent, execution, &config.id)?;
        let (agent, execution, provider) = (agent.clone(), execution.clone(), shard.id.clone());
        let (fork_from, child_thread) = (
            config.history.fork_source().map(str::to_owned),
            config.id.clone(),
        );
        let generation = lease.generation();
        pool.hub.blocking_with(move |core| {
            let mut current = core.store().agent(&agent)?;
            if current.execution_id.as_deref() != Some(&execution)
                || current.auth_generation != generation
                || current.thread_id.as_deref() != Some(&child_thread)
            {
                return Err(Error::stale_execution("Launch no longer owns this agent."));
            }
            let intent = core.store().fork_intent(&current)?;
            if intent.as_ref().map(|b| &b.source_thread) != fork_from.as_ref() {
                return Err(Error::state_inconsistent(
                    "Native fork launch differs from its saved source.",
                ));
            }
            if fork_from.is_some() {
                core.store().begin_branch(&mut current)?;
            }
            current.provider_process = Some(ProviderProcess::Reserved { id: provider });
            current.conversation_clean = false;
            core.store().save(&current)
        })?;
    }

    Ok(())
}

fn settle_unstarted_branch(
    pool: &Pool,
    shard: &Shard,
    owner: Option<&LaunchOwner>,
) -> Result<bool> {
    let Some(LaunchOwner { agent, execution }) = owner else {
        return Ok(false);
    };
    let claude_fork = matches!(
        &shard.backend,
        Backend::Claude(config)
            if config.history.fork_source().is_some()
    );
    let (agent, execution, provider) = (agent.clone(), execution.clone(), shard.id.clone());
    pool.hub.blocking_with(move |core| {
        let mut child = core.store().agent(&agent)?;
        if child.execution_id.as_deref() != Some(&execution) {
            return Ok(false);
        }
        if child.branch_phase == Some(BranchPhase::ReadyToFork) {
            return core
                .store()
                .fork_intent(&child)
                .map(|intent| intent.is_some());
        }
        if !claude_fork || child.provider_id() != Some(&provider) {
            return Ok(false);
        }
        child.conversation_clean = true;
        core.store()
            .branch_creation_rejected(&mut child, BranchFailure::NativeNotStarted)
    })
}

#[cfg(test)]
mod failure_tests;
