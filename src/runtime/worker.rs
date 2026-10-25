//! Run one claimed worker execution and persist its lifecycle boundaries.

mod failure;
use failure::finish_failure;
#[cfg(test)]
use failure::record_failure;

mod preparation;
use preparation::{execution_backend, provider_command, reserve_execution};

use crate::runtime::domain::ProviderProcess;
use crate::runtime::provider::ShutdownMethod;
mod execution;
mod session;

use crate::runtime::api::{self, events};
use execution::ExecutionHandle;

use super::{
    Core, Shared,
    branching::{BranchFailure, BranchPhase, Intent},
    domain::{Agent, AgentState, Control, EventKind, RecoveryFence},
    preflight,
    provider::{
        Backend, Launch, LaunchOwner, Pool,
        claude::{History, IdentityPrompt, SessionConfig},
    },
};
use crate::{
    config::Provider,
    error::{Error, Result},
    process,
    state::{Lock, RuntimeLease},
};
use session::execute_session;

// Back off after a safely rejected branch creation before starting a new execution.
const EXECUTION_RETRY_DELAY: Duration = Duration::from_secs(5);
use std::{
    path::Path,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

pub(crate) struct WorkerInputs {
    pub shared: Shared,
    pub id: String,
    pub pool: Pool,
    pub notify: Arc<Notify>,
    pub starting: OwnedSemaphorePermit,
    pub turns: Arc<Semaphore>,
}
struct ExecutionInputs {
    pool: Pool,
    notify: Arc<Notify>,
    starting: OwnedSemaphorePermit,
    turns: Arc<Semaphore>,
}

pub(crate) async fn run(inputs: WorkerInputs) {
    let WorkerInputs {
        shared,
        id,
        pool,
        notify,
        starting,
        turns,
    } = inputs;
    let mut cleaned = true;
    let result = Box::pin(execute(
        &shared,
        &id,
        ExecutionInputs {
            pool,
            notify,
            starting,
            turns,
        },
        &mut cleaned,
    ))
    .await;
    if let Err(error) = result {
        finish_failure(&shared, id, error, cleaned).await;
    }
}

struct PreparedExecution {
    agent: Agent,
    execution: String,
    lease: Arc<RuntimeLease>,
    command: Launch,
    daemon_lock: Arc<Lock>,
    shutdown: Arc<AtomicBool>,
    idle_seconds: u64,
    placement: usize,
}
fn executable_agent(core: &mut Core, key: &str) -> Result<Option<Agent>> {
    let mut agent = core.store().agent(key)?;
    if !super::features::available(&agent) {
        return Ok(None);
    }
    if agent.waiting_reason.is_some()
        || agent.branch_hold > 0
        || agent
            .branch_phase
            .is_some_and(|p| !matches!(p, BranchPhase::ReadyToFork | BranchPhase::Bound))
        || agent.terminal()
        || agent.paused
        || agent.control == Some(Control::Stop)
        || core.shutdown().load(Ordering::Relaxed)
    {
        agent.state = AgentState::Stopped;
        agent.control = None;
        core.store().save(&agent)?;
        return Ok(None);
    }
    Ok(Some(agent))
}
enum Preparation {
    Refresh,
    Ready(Option<Box<PreparedExecution>>),
}
fn prepare_execution(
    core: &mut Core,
    key: &str,
    proof: preflight::WorkerProof,
) -> Result<Preparation> {
    let Some(mut agent) = executable_agent(core, key)? else {
        return Ok(Preparation::Ready(None));
    };
    if preflight::Identity::from(&agent) != proof.identity {
        return Ok(Preparation::Refresh);
    }
    let preflight::ExecutionProfile {
        lease,
        dir,
        binary,
        steering,
        credential,
        compatibility,
    } = proof.checked?;
    crate::options::catalog(agent.profile.provider)?.validate_version(
        &agent.boundary_selection().options.values,
        &compatibility.observation.version,
    )?;
    if !credential.fresh() {
        return Ok(Preparation::Refresh);
    }
    if let Some(key) = credential.authority() {
        core.store()
            .select_key_authority(&agent.auth, agent.auth_generation, Some(key))?;
    }
    agent.execution_key = credential.authority().cloned();
    // This only reads intent. Claude begins at the gated exec authorization;
    // Codex begins immediately before thread/fork.
    let branch = core.store().fork_intent(&agent)?;
    let mut command = provider_command(&binary, agent.profile.provider, &dir, &lease);
    credential.configure(agent.profile.provider, &mut command);
    let execution = Uuid::new_v4().to_string();
    let (selection, process_options) =
        reserve_execution(core, &mut agent, &execution, &mut command)?;
    agent.instructions = core.store().instructions(&agent.id)?;
    let placement = core.store().placement(&agent.id)?;
    let backend = execution_backend(&agent, selection, branch.as_ref(), steering)?;
    let command = Launch {
        process_options,
        compatibility: Some(compatibility),
        credential,
        command,
        launcher: core.launcher().clone(),
        backend,
        owner: Some(LaunchOwner {
            agent: agent.id.clone(),
            execution: execution.clone(),
        }),
    };
    Ok(Preparation::Ready(Some(Box::new(PreparedExecution {
        agent,
        execution,
        lease,
        command,
        daemon_lock: core.daemon_lock().clone(),
        shutdown: core.shutdown().clone(),
        idle_seconds: core.config().runtime.idle_seconds,
        placement,
    }))))
}

async fn prepare(shared: &Shared, id: &str) -> Result<Option<PreparedExecution>> {
    for _ in 0..preflight::MAX_ATTEMPTS {
        let key = id.to_owned();
        let input = shared
            .with(move |core| {
                let Some(agent) = executable_agent(core, &key)? else {
                    return Ok(None);
                };
                Ok(Some(preflight::WorkerInput {
                    launcher: core.launcher().clone(),
                    state: core.state().clone(),
                    profile: core.config().profile(&agent.auth).cloned(),
                    binary: core.binaries().resolve(agent.profile.provider),
                    agent,
                }))
            })
            .await?;
        let Some(input) = input else {
            return Ok(None);
        };
        let proof = shared.preflight.worker(input).await?;
        let key = id.to_owned();
        match shared
            .with(move |core| prepare_execution(core, &key, proof))
            .await?
        {
            Preparation::Refresh => {}
            Preparation::Ready(prepared) => return Ok(prepared.map(|prepared| *prepared)),
        }
    }
    Err(Error::stale_execution(
        "Agent changed repeatedly during execution preparation.",
    ))
}
// Keep both permits and the claimed session in this scope: a failed startup record
// must still close the provider before initialization ownership is released.
async fn execute(
    shared: &Shared,
    id: &str,
    inputs: ExecutionInputs,
    cleaned: &mut bool,
) -> Result<()> {
    let ExecutionInputs {
        pool,
        notify,
        starting,
        turns,
    } = inputs;
    let prepared = prepare(shared, id).await?;
    let Some(PreparedExecution {
        agent,
        execution,
        lease,
        command,
        daemon_lock,
        shutdown,
        idle_seconds,
        placement,
    }) = prepared
    else {
        return Ok(());
    };
    let mut rpc = pool
        .acquire(super::provider::SessionRequest {
            agent: id,
            previous_provider: agent.thread_id.as_ref().and(agent.provider_id()),
            auth: &agent.auth,
            command,
            lease,
            daemon_lock,
            class: placement,
        })
        .await?;
    *cleaned = false;
    let provider_id = rpc.provider_id().to_owned();
    let started = rpc.started.clone();
    let execution = ExecutionHandle::new(shared, id, &execution);
    let recorded = record_provider_started(execution, provider_id, started).await;
    let mut turn_permit = None;
    let result = match recorded {
        Ok(()) => {
            Box::pin(execute_session(
                execution,
                session::SessionInputs {
                    agent,
                    rpc: &mut rpc,
                    shutdown,
                    notify,
                    starting,
                    turns,
                    permit: &mut turn_permit,
                    idle_seconds,
                },
            ))
            .await
        }
        Err(e) => Err(e),
    };
    close_execution(execution, &mut rpc, turn_permit, result, cleaned).await
}
async fn close_execution(
    execution: ExecutionHandle<'_>,
    rpc: &mut super::provider::Session,
    turn_permit: Option<OwnedSemaphorePermit>,
    result: Result<bool>,
    cleaned: &mut bool,
) -> Result<()> {
    let drained = result
        .as_ref()
        .is_err_and(|e| matches!(e, Error::ProviderDraining { .. }));
    let result = if drained { Ok(false) } else { result };
    let cleanup = rpc.close().await;
    *cleaned = cleanup.confirmed;
    drop(turn_permit);
    finish_execution(execution, result, cleanup, drained).await
}

async fn finish_execution(
    execution: ExecutionHandle<'_>,
    result: Result<bool>,
    cleanup: super::provider::Shutdown,
    drained: bool,
) -> Result<()> {
    let value = cleanup.clone();
    let outcome = result.clone();
    let recorded = record_execution_stopped(execution, value, outcome, drained).await;
    let result = crate::observability::settle(result, recorded, "execution_stop_record");
    let proof = if cleanup.confirmed {
        Ok(())
    } else {
        Err(Error::recovery_required(
            "Provider cleanup could not be confirmed.",
        ))
    };
    // The caller retains cleanup.confirmed independently, so the primary error
    // can survive without weakening the recovery fence for uncertain cleanup.
    crate::observability::settle(result, proof, "execution_cleanup").map(|_| ())
}
async fn record_provider_started(
    execution: ExecutionHandle<'_>,
    provider_id: String,
    started: super::provider::Started,
) -> Result<()> {
    execution
        .agent(move |core, agent| {
            agent.provider_process = Some(ProviderProcess::Started {
                id: provider_id.clone(),
                pid: started.pid,
                before: started.before,
                after: started.after,
            });
            agent.conversation_clean = false;
            core.store().save(agent)?;
            core.store().event(
                agent,
                EventKind::ProviderStarted,
                api::value(events::ProviderStarted {
                    provider_id: &provider_id,
                    pgid: started.pid,
                    process: events::ProcessStart {
                        pid: started.pid,
                        spawn_before: started.before,
                        spawn_after: started.after,
                    },
                }),
            )
        })
        .await
}
#[expect(
    clippy::expect_used,
    reason = "ExecutionHandle::agent validates the live execution ID before entering this \
        closure."
)]
async fn record_execution_stopped(
    execution: ExecutionHandle<'_>,
    value: super::provider::Shutdown,
    outcome: Result<bool>,
    drained: bool,
) -> Result<()> {
    execution
        .agent(move |core, agent| {
            core.store().expire_steering(&agent.id)?;
            if agent.redirect_pending {
                core.store().lose_redirects(
                    &agent.id,
                    agent.execution_id.as_deref().expect("active execution"),
                )?;
                agent.redirect_pending = false;
            }
            agent.disable_steering();
            if value.method == ShutdownMethod::SessionReleased {
                core.store().event(
                    agent,
                    EventKind::ProviderSessionReleased,
                    api::value(&value),
                )?;
            }
            core.store()
                .record_conversation_release(agent, value.confirmed)?;
            if value.confirmed {
                if drained || outcome.as_ref().is_err_and(Error::input_was_not_submitted) {
                    core.store().defer_submission(&agent.id)?;
                } else if outcome
                    .as_ref()
                    .is_err_and(|e| matches!(e, Error::ProviderRequestFailed { .. }))
                {
                    core.store().reject_submission(&agent.id)?;
                }
            }
            if let Ok(unfinished) = outcome {
                if unfinished {
                    core.auth_uncertain_cutoff(agent)?;
                    core.store().uncertain(&agent.id)?;
                }
                agent.state = AgentState::Stopped;
                agent.clear_turn();
                agent.control = None;
                core.store().save(agent)?;
                core.store().event(
                    agent,
                    EventKind::ExecutionStopped,
                    api::value(events::ExecutionStopped::Outcome {
                        task_outcome: agent.task_outcome,
                    }),
                )?;
            }
            Ok(())
        })
        .await
}

#[cfg(test)]
mod preflight_tests;

#[cfg(test)]
mod failure_tests;
