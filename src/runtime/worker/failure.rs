//! Failure for this component.
use super::api;
use super::{
    AgentState, BranchFailure, BranchPhase, Core, EXECUTION_RETRY_DELAY, Error, EventKind,
    RecoveryFence, Result, Shared, events,
};

pub(super) async fn finish_failure(shared: &Shared, id: String, error: Error, cleaned: bool) {
    tracing::error!(name: "worker.execution_failed",
         agent_id = id.as_str(),
         code = error.code(),
         error = &error as &(dyn std::error::Error + 'static),
         confirmed = cleaned,
         decision = "record_failure");
    let original = error.clone();
    let agent_id = id.clone();
    let recorded = shared
        .with(move |core| record_failure(core, &id, error, cleaned))
        .await;
    match recorded {
        Ok(true) => {
            // Cleanup released provider/turn permits; retain the worker claim briefly
            // so a safely rejected pre-exec branch cannot spin repeated launches.
            tokio::time::sleep(EXECUTION_RETRY_DELAY).await;
        }
        Ok(false) => {}
        Err(secondary) => {
            shared.stop_after_failure(&secondary);
            tracing::error!(name: "worker.failure_record_failed",
                 agent_id = agent_id.as_str(),
                 code = secondary.code(),
                 primary = &original as &(dyn std::error::Error + 'static),
                 secondary = &secondary as &(dyn std::error::Error + 'static),
                 sqlite_code = super::super::store::sqlite_code(&secondary),
                 decision = "shutdown_retain_evidence");
        }
    }
}

pub(super) fn record_failure(
    core: &mut Core,
    id: &str,
    error: Error,
    cleaned: bool,
) -> Result<bool> {
    let mut agent = core.store().agent(id)?;
    if cleaned
        && crate::state::is_key_changed(&error)
        && !agent.terminal()
        && !agent.paused
        && agent.control.is_none()
    {
        agent.state = AgentState::Stopped;
        agent.error = None;
        core.store().save(&agent)?;
        return Ok(true);
    }
    if cleaned
        && error.input_was_not_submitted()
        && agent.branch_phase == Some(BranchPhase::ReadyToFork)
        && agent.recovery_fence.is_none()
        && !agent.terminal()
        && !agent.paused
        && agent.control.is_none()
    {
        agent.state = AgentState::Stopped;
        agent.error = None;
        core.store().save(&agent)?;
        core.store().event(
            &agent,
            EventKind::ExecutionStopped,
            api::value(events::ExecutionStopped::Unsubmitted {
                reason: "branch_creation_not_submitted",
                error: &error,
            }),
        )?;
        return Ok(true);
    }
    if !cleaned || matches!(error, Error::RecoveryRequired { .. }) {
        agent.recovery_fence = Some(RecoveryFence::Failure(error.to_string()));
    }
    agent.state = if cleaned && agent.terminal() {
        AgentState::Stopped
    } else {
        AgentState::Failed
    };
    agent.error = Some(error.code().into());
    agent.disable_steering();
    core.auth_uncertain_cutoff(&mut agent)?;
    let failed_turn = agent.turn_id.clone();
    agent.clear_turn();
    agent.control = None;
    agent.completion_pending = None;
    core.store().uncertain(id)?;
    core.store().save(&agent)?;
    if agent.branch_unbound() {
        core.store()
            .fail_branch(&mut agent, BranchFailure::Error(error.kind()), false)?;
    }
    core.store().event(
        &agent,
        EventKind::ExecutionFailed,
        api::value(events::ExecutionFailure {
            error: &error,
            turn_id: failed_turn.as_deref(),
        }),
    )?;
    Ok(false)
}
