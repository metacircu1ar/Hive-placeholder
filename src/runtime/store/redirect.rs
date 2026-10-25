//! Priority next-turn input and an independently fenced interruption target.
mod queries;
use super::{Store, api};
use crate::{
    error::Result,
    runtime::domain::{Agent, RedirectState},
};
use rusqlite::params;

impl Store {
    pub fn enqueue_redirect(
        &self,
        id: &str,
        sender: &str,
        agent: &mut Agent,
        body: &str,
    ) -> Result<Option<api::RedirectTarget>> {
        let target = self.redirect_target(agent)?;
        self.enqueue_ordered(id, sender, &agent.id, body, false, true)?;
        if let Some(target) = &target {
            // One execution/input can be interrupted at most once, even when
            // multiple distinct redirect requests target the same active turn.
            let inserted = self.execute_cached(
                queries::ENQUEUE_REDIRECT,
                params![
                    agent.id,
                    target.execution_id,
                    target.message_id,
                    target.turn_id,
                    RedirectState::Pending
                ],
            )?;
            if inserted != 0 {
                agent.redirect_pending = true;
                self.save(agent)?;
            }
        }
        Ok(target)
    }

    fn redirect_target(&self, agent: &Agent) -> Result<Option<api::RedirectTarget>> {
        let Some(execution) = &agent.execution_id else {
            return Ok(None);
        };
        let message: Option<String> = self.query_opt_cached(
            queries::REDIRECT_TARGET,
            params![
                agent.id,
                execution,
                agent.reserved_turn.as_ref().map(|r| &r.message_id)
            ],
            |row| row.get(0),
        )?;
        Ok(message.map(|message_id| api::RedirectTarget {
            execution_id: execution.clone(),
            message_id,
            turn_id: agent.turn_id.clone(),
        }))
    }

    /// Mark before the native call: a crash must never turn a retry into a second
    /// interruption. Recovery loses the old target and never replays its input.
    pub fn claim_redirect(
        &self,
        agent: &str,
        execution: &str,
        message: &str,
        turn: &str,
    ) -> Result<bool> {
        self.execute_cached(
            queries::CLAIM_REDIRECT,
            params![agent, execution, message, turn, RedirectState::Sent],
        )
        .map(|changed| changed != 0)
    }

    pub fn finish_redirect(&self, execution: &str, message: &str) -> Result<()> {
        self.execute_cached(
            queries::FINISH_REDIRECT,
            params![execution, message, RedirectState::Resolved],
        )?;
        Ok(())
    }

    pub fn lose_redirects(&self, agent: &str, execution: &str) -> Result<()> {
        self.execute_cached(
            queries::LOSE_REDIRECTS,
            params![agent, execution, RedirectState::TargetLost],
        )?;
        Ok(())
    }
}
