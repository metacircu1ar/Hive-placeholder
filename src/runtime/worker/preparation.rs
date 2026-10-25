//! Preparation for this component.
use super::api;
use super::{
    Agent, AgentState, Arc, Backend, Command, Core, Error, EventKind, History, IdentityPrompt,
    Intent, Path, Provider, Result, RuntimeLease, SessionConfig, events, process, session,
};

pub(super) fn configure_process_options(
    command: &mut Command,
    provider: Provider,
    options: &crate::options::Snapshot,
) -> Result<String> {
    if provider == Provider::Codex
        && let Some(value) = options.values.get("codex.request_compression")
    {
        command.args([
            "-c",
            &format!("features.enable_request_compression={value}"),
        ]);
    }
    options.process_key(provider)
}

pub(super) fn provider_command(
    binary: &Path,
    provider: Provider,
    dir: &Path,
    lease: &RuntimeLease,
) -> Command {
    let mut command = process::command(binary, provider, &dir.join("provider"), &dir.join("work"));
    lease.configure(&mut command);
    if provider == Provider::Codex {
        command.args([
            "-c",
            "features.multi_agent=false",
            "-c",
            "features.memories=false",
            "-c",
            "features.goals=false",
        ]);
    }
    command
}

pub(super) fn execution_backend(
    agent: &Agent,
    selection: super::super::domain::OptionsSelection,
    branch: Option<&Intent>,
    steering: Option<super::super::steering_canary::Candidate>,
) -> Result<Backend> {
    let backend = if agent.profile.provider == Provider::Claude {
        Backend::Claude(Arc::new(SessionConfig {
            tool_catalog: agent.tool_catalog,
            comb_reviewer: agent.comb.as_ref().map(|b| b.reviewer),
            mcp_servers: agent.mcp_servers.clone(),
            id: agent.thread_id.clone().ok_or(Error::session_mismatch(
                "Missing Claude session reservation.",
            ))?,
            history: if let Some(branch) = branch {
                History::Fork {
                    cutoff: branch.source_turn.clone(),
                    source: branch.source_thread.clone(),
                }
            } else {
                let prompt = if agent.branched_from_agent_id.is_some() {
                    IdentityPrompt::CurrentAgent
                } else {
                    IdentityPrompt::NativeSnapshot
                };
                if agent.native_input_attempted {
                    History::Resume(prompt)
                } else {
                    History::New(prompt)
                }
            },
            model: selection.model,
            effort: selection.effort,
            options: selection.options,
            cwd: agent.cwd.clone(),
            instructions: session::identity(agent),
            sandbox: agent.sandbox,
            steering: None,
            capability: steering,
        }))
    } else {
        Backend::Codex
    };
    Ok(backend)
}

pub(super) fn reserve_execution(
    core: &Core,
    agent: &mut Agent,
    execution: &str,
    command: &mut Command,
) -> Result<(super::super::domain::OptionsSelection, String)> {
    agent.execution_id = Some(execution.to_owned());
    agent.disable_steering();
    agent.state = AgentState::Starting;
    agent.error = None;
    core.store().reserve_turn(agent)?;
    let selection = agent.boundary_selection();
    let process_options =
        configure_process_options(command, agent.profile.provider, &selection.options)?;
    // Startup verifies this reservation against the selected process catalog
    // before any input. A newer desired selection keeps its own catalog authority.
    if selection.revision == agent.options_revision {
        agent.model_key.clone_from(&agent.execution_key);
    }
    core.store().save(agent)?;
    core.store().event(
        agent,
        EventKind::ExecutionStarting,
        api::value(events::ExecutionStarting {
            model: &selection.model,
            effort: &selection.effort,
            options_revision: selection.revision,
        }),
    )?;
    Ok((selection, process_options))
}
