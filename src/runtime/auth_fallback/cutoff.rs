//! Authentication cutoffs and native turn-boundary requests.
use super::AuthReason;
use super::{SwitchIntent, WaitingReason};
use crate::runtime::api::{self, events};
use crate::{
    error::Result,
    runtime::{
        Core,
        domain::{Agent, EventKind},
    },
};
use sha2::{Digest, Sha256};

impl Core {
    pub(in crate::runtime) fn request_auth_boundary(
        &self,
        agent: &mut Agent,
        target: String,
        reason: AuthReason,
    ) -> Result<()> {
        if agent.terminal() || agent.branch_hold > 0 || agent.branch_unbound() {
            return Ok(());
        }
        agent.waiting_reason = Some(WaitingReason::Switch);
        agent.quota_wait_until = None;
        agent.auth_switch_pending = Some(SwitchIntent {
            target,
            reason,
            source_generation: agent.auth_generation,
            source_execution: agent.execution_id.clone(),
        });
        self.store().save(agent)?;
        self.store().deadlines_changed();
        Ok(())
    }

    pub(in crate::runtime) fn auth_uncertain_cutoff(&self, agent: &mut Agent) -> Result<()> {
        if let Some(crate::runtime::domain::AuthCutoff::Bound { turn, message }) =
            agent.auth_cutoff.clone()
        {
            if self.store().cutoff_input(agent, &turn)?.as_deref() == Some(message.as_str()) {
                self.auth_cutoff(agent, &message, &turn)?;
            }
            agent.clear_cutoff();
        }
        Ok(())
    }

    #[expect(
        clippy::expect_used,
        reason = "The continuation tuple contains only strings and a count."
    )]
    pub(in crate::runtime) fn auth_cutoff(
        &self,
        agent: &mut Agent,
        message: &str,
        turn: &str,
    ) -> Result<()> {
        if (agent.auth_group.is_none() && agent.profile.method != crate::config::Method::ApiKey)
            || agent.terminal()
        {
            return Ok(());
        }
        if agent.auth_switch_pending.is_none() {
            self.request_auth_boundary(
                agent,
                agent
                    .auth_group
                    .clone()
                    .unwrap_or_else(|| agent.auth.clone()),
                AuthReason::UsageLimit,
            )?;
        }
        // Identity is tied to the interrupted input, not subsequent wait/switch decisions.
        let origin = self.store().continuation_origin(&agent.id, message)?;
        let root = origin.root;
        let attempt = origin.attempt;
        let identity =
            serde_json::to_vec(&(&agent.id, &root, attempt)).expect("continuation identity");
        let id = format!("continuation:{:x}", Sha256::digest(identity));
        let body = format!(
            concat!(
                "Your previous turn was cut off by a login limit ",
                "(message {root}, continuation attempt {attempt}). ",
                "Check your messages and the login handoff in your instructions, ",
                "if present, and continue your task. Do not reply to this message."
            ),
            root = root,
            attempt = attempt,
        );
        if self
            .store()
            .enqueue_continuation(&id, &agent.id, &body, &root, attempt)?
        {
            self.store().event(
                agent,
                EventKind::ContinuationQueued,
                api::value(events::ContinuationQueued {
                    message_id: &id,
                    interrupted_message_id: message,
                    original_message_id: &root,
                    attempt,
                    turn_id: turn,
                }),
            )?;
        }
        Ok(())
    }
}
