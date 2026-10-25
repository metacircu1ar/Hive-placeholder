//! Model reservation and the final input submission boundary.
use super::{
    Agent, Error, Message, MessageState, Model, OptionsSelection, Ordering, Response, Result,
};
use crate::{
    config::Provider,
    runtime::{
        Core,
        branching::{AttemptedTurn, BranchPhase},
        store::TurnReservation,
    },
};

pub(super) fn session_selection(agent: &Agent, codex: bool) -> Result<OptionsSelection> {
    // Restore a known Codex selection before applying the next reserved one.
    // A saved thread without a verified selection belongs
    // to an interrupted/failed initial handshake, so verify its reservation.
    if codex && agent.thread_id.is_some() {
        let restored = agent
            .last_accepted_selection
            .as_ref()
            .or(agent.session_selection.as_ref())
            .cloned()
            .unwrap_or_else(|| agent.boundary_selection());
        let desired = agent.boundary_selection();
        if crate::options::same_scope(
            agent.profile.provider,
            crate::options::Scope::Session,
            &restored.options,
            &desired.options,
        )? && crate::options::same_scope(
            agent.profile.provider,
            crate::options::Scope::Process,
            &restored.options,
            &desired.options,
        )? {
            Ok(restored)
        } else {
            Ok(desired)
        }
    } else {
        Ok(agent.boundary_selection())
    }
}

pub(super) struct VerifiedSelection {
    pub selection: OptionsSelection,
    pub source: &'static str,
}

pub(super) fn verified_selection(
    agent: &Agent,
    requested: &OptionsSelection,
    response: &Response,
    catalog: &[Model],
    codex: bool,
) -> Result<VerifiedSelection> {
    let restored = if codex && agent.thread_id.is_some() {
        agent.session_selection.as_ref().filter(|known| {
            response.model() == Some(known.model.as_str())
                && response.effort() == Some(known.effort.as_str())
        })
    } else {
        None
    };
    let selected = if response.model() == Some(requested.model.as_str())
        && response.effort() == Some(requested.effort.as_str())
    {
        requested
    } else {
        restored.unwrap_or(requested)
    };
    super::validate_model(&selected.model, &selected.effort, catalog)?;
    let expected_model = if codex {
        Some(selected.model.as_str())
    } else {
        catalog
            .iter()
            .find(|m| m.name() == Some(&selected.model))
            .and_then(Model::resolved_name)
    };
    if response.model() != expected_model {
        return Err(Error::model_mismatch(
            "Provider selected an unrequested model; execution refused.",
        ));
    }
    if response.effort() != Some(&selected.effort) {
        return Err(Error::effort_mismatch(
            "Provider selected an unrequested model/effort pair; execution refused.",
        ));
    }
    let source = if codex && agent.thread_id.is_some() {
        if agent.last_accepted_selection.as_ref() == Some(selected) {
            "last_accepted_turn"
        } else if agent.session_selection.as_ref() == Some(selected) {
            "last_verified_session"
        } else {
            "reserved"
        }
    } else {
        "reserved"
    };
    // The resume echo verifies the model pair, not the newly established
    // session/process settings. Keep those settings and their revision even
    // when Codex restores the thread's original model pair.
    let mut selection = requested.clone();
    if selected.model != requested.model || selected.effort != requested.effort {
        selection.model.clone_from(&selected.model);
        selection.effort.clone_from(&selected.effort);
        let options = std::sync::Arc::make_mut(&mut selection.options);
        for (key, value) in [("model", &selected.model), ("effort", &selected.effort)] {
            options.values.insert(key.into(), value.clone().into());
            if let Some(source) = selected.options.sources.get(key) {
                options.sources.insert(key.into(), source.clone());
            }
        }
    }
    Ok(VerifiedSelection { selection, source })
}

pub(super) enum TurnAdmission {
    None,
    Relaunch,
    Submit {
        message: Box<Message>,
        selection: OptionsSelection,
        previously_attempted: bool,
    },
}

#[expect(
    clippy::expect_used,
    reason = "Only the bound session loop claims work, inside ExecutionHandle::agent with \
        its live execution fence."
)]
pub(super) fn claim(
    core: &Core,
    agent: &mut Agent,
    loaded: &OptionsSelection,
    provider_version: Option<&str>,
) -> Result<TurnAdmission> {
    if claim_blocked(core, agent) {
        return Ok(TurnAdmission::None);
    }
    let previously_attempted = agent.native_input_attempted;
    let Some(TurnReservation {
        mut message,
        selection,
    }) = core.store().reserve_turn(agent)?
    else {
        return Ok(TurnAdmission::None);
    };
    if let Some(version) = provider_version {
        crate::options::catalog(agent.profile.provider)?
            .validate_version(&selection.options.values, version)?;
    }
    if !core.auth_admit(agent, &message)? {
        return Ok(TurnAdmission::None);
    }
    if requires_relaunch(agent, loaded, &selection)? {
        core.store().save(agent)?;
        return Ok(TurnAdmission::Relaunch);
    }
    core.store().plan_message_images(&mut message)?;
    agent.native_input_attempted = true;
    agent.last_attempted_turn = Some(AttemptedTurn {
        message: message.id.clone(),
        thread: agent.thread_id.clone().expect("ready session"),
        execution: agent.execution_id.clone().expect("ready execution"),
    });
    agent.clear_cutoff();
    agent.turn_selection = Some(selection.clone());
    agent.turn_auth_revision = Some(
        core.store()
            .availability(&agent.auth, agent.auth_generation)?
            .revision,
    );
    core.store()
        .message_state(&message, MessageState::Submitting)?;
    core.store().save(agent)?;
    Ok(TurnAdmission::Submit {
        message: Box::new(message),
        selection,
        previously_attempted,
    })
}

fn claim_blocked(core: &Core, agent: &Agent) -> bool {
    agent.waiting_reason.is_some()
        || agent.branch_hold > 0
        || (agent.branch_unbound()
            && !(agent.profile.provider == Provider::Claude
                && agent.branch_phase == Some(BranchPhase::Forking)
                && !agent.native_input_attempted))
        || agent.paused
        || agent.terminal()
        || agent.control.is_some()
        || agent.completion_pending.is_some()
        || core.shutdown().load(Ordering::Relaxed)
}

fn requires_relaunch(
    agent: &Agent,
    loaded: &OptionsSelection,
    selection: &OptionsSelection,
) -> Result<bool> {
    let session_changed = !crate::options::same_scope(
        agent.profile.provider,
        crate::options::Scope::Session,
        &loaded.options,
        &selection.options,
    )?;
    let process_changed = !crate::options::same_scope(
        agent.profile.provider,
        crate::options::Scope::Process,
        &loaded.options,
        &selection.options,
    )?;
    Ok(
        (selection.revision == agent.options_revision && agent.model_key != agent.execution_key)
            || session_changed
            || process_changed,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn restored_model_keeps_new_session_settings_and_revision() {
        let mut agent = crate::runtime::test_support::AgentBuilder::new("a")
            .configure(|a| {
                a.model = "terra".into();
                a.effort = "high".into();
                a.thread_id = Some("thread".into());
            })
            .build();
        let original = agent.desired_selection();
        agent.session_selection = Some(original.clone());
        let mut requested = original;
        requested.model = "luna".into();
        requested.effort = "medium".into();
        requested.revision = 2;
        let settings = std::sync::Arc::make_mut(&mut requested.options);
        for (key, value) in [
            ("model", json!("luna")),
            ("effort", json!("medium")),
            ("codex.tool_output_token_limit", json!(1000)),
        ] {
            settings.values.insert(key.into(), value);
            settings.sources.insert(
                key.into(),
                crate::options::OptionSource::plain("set_options"),
            );
        }
        let catalog = [Model::parse(
            &json!({"model":"terra", "supportedReasoningEfforts":[{"reasoningEffort":"high"}]}),
        )];
        let response = Response::parse(json!({"model":"terra", "reasoningEffort":"high"}));
        let verified = verified_selection(&agent, &requested, &response, &catalog, true).unwrap();
        assert_eq!(
            (&*verified.selection.model, &*verified.selection.effort),
            ("terra", "high")
        );
        assert_eq!(verified.selection.revision, 2);
        assert_eq!(verified.selection.options.string("model"), Some("terra"));
        for scope in [
            crate::options::Scope::Session,
            crate::options::Scope::Process,
        ] {
            assert!(
                crate::options::same_scope(
                    Provider::Codex,
                    scope,
                    &verified.selection.options,
                    &requested.options
                )
                .unwrap()
            );
        }
        let unexpected = Response::parse(json!({"model":"other", "reasoningEffort":"high"}));
        assert!(verified_selection(&agent, &requested, &unexpected, &catalog, true).is_err());
    }
}
