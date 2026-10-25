//! Agent-owned history deletion. File cleanup follows the committed deletion outbox.
mod queries;
pub(in crate::runtime) use super::artifacts::{ArtifactDeletions, unlink_artifacts};
use super::{Agent, Queries, Store, api};
use crate::{
    error::{Error, Result},
    runtime::domain::{AgentState, PurgeBlockerReason},
};
use rusqlite::params;
use std::collections::HashSet;

const PREVIEW: usize = 20;

impl Queries {
    pub(super) fn purge_blocker(
        &self,
        a: &Agent,
        claimed: bool,
        blocked: bool,
    ) -> Result<Option<PurgeBlockerReason>> {
        if a.branch_hold > 0 {
            return Ok(Some(PurgeBlockerReason::PendingNativeBranches));
        }
        if !a.terminal() {
            return Ok(Some(PurgeBlockerReason::AgentNotEnded));
        }
        if a.state != AgentState::Stopped
            || claimed
            || a.control.is_some()
            || a.turn_id.is_some()
            || a.completion_pending.is_some()
        {
            return Ok(Some(PurgeBlockerReason::AgentCleanupPending));
        }
        let never_launched = a.execution_id.is_none()
            && a.provider_process.is_none()
            && (a.thread_id.is_none() || a.profile.provider == crate::config::Provider::Claude)
            && !a.native_input_attempted;
        if blocked || a.recovery_fence.is_some() || !(a.conversation_clean || never_launched) {
            return Ok(Some(PurgeBlockerReason::AgentCleanupUnconfirmed));
        }
        let active: bool = self
            .sql()
            .query_row(queries::HAS_PENDING_INPUT, [&a.id], |r| r.get(0))?;
        if active {
            return Ok(Some(PurgeBlockerReason::ActiveInput));
        }
        if let Some(reserved) = &a.reserved_turn {
            let terminal: Option<bool> = self.sql().query_opt(
                queries::RESERVED_INPUT_TERMINAL,
                params![a.id, reserved.message_id],
                |r| r.get(0),
            )?;
            if terminal != Some(true) {
                return Ok(Some(PurgeBlockerReason::UnresolvedReservation));
            }
        }
        Ok(None)
    }

    pub(super) fn was_purged(&self, id: &str) -> Result<bool> {
        self.query_row_cached(queries::WAS_PURGED, [id], |r| r.get(0))
    }

    /// Only historical references may use this; normal lookups report `agent_purged`.
    pub(in crate::runtime) fn agent_or_purged(&self, id: &str) -> Result<Option<Agent>> {
        match self.agent(id) {
            Ok(agent) => Ok(Some(agent)),
            Err(Error::AgentPurged { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl Store {
    pub(in crate::runtime) fn purge(
        &self,
        target: &str,
        recursive: bool,
        dry_run: bool,
        running: &HashSet<String>,
    ) -> Result<api::PurgeResponse> {
        self.prepare_purge_scope(target, recursive)?;
        let mut response = api::PurgeResponse {
            target: target.into(),
            dry_run,
            recursive,
            selected_agents: 0,
            selected_preview: vec![],
            blocked_agents: 0,
            blockers: vec![],
            counts: api::PurgeCounts::default(),
            native_history: "retained",
            retained: concat!(
                "Survivor messages, events and replay records; identity/lineage ",
                "tombstones; shared provider and recovery evidence; native ",
                "conversations; working directories",
            ),
        };
        self.collect_purge_candidates(running, &mut response)?;
        if !dry_run && response.blocked_agents != 0 {
            return Err(Error::agent_busy(concat!(
                "Purge requires every selected agent to be ended and safely released. ",
                "Use --dry-run to inspect blockers.",
            )));
        }
        response.counts = self.purge_counts()?;
        if !dry_run {
            self.delete_purge_scope()?;
        }
        self.clear_purge_scope()?;
        if !dry_run {
            self.maintenance_notice(false, response.selected_agents as u64);
        }
        Ok(response)
    }
    fn collect_purge_candidates(
        &self,
        running: &HashSet<String>,
        response: &mut api::PurgeResponse,
    ) -> Result<()> {
        let mut after = String::new();
        loop {
            let rows: Vec<super::rows::PurgeAgent> =
                self.sql().cached().query_rows(queries::PURGE, [&after])?;
            if rows.is_empty() {
                break;
            }
            for super::rows::PurgeAgent {
                id,
                data: raw,
                claimed,
                blocked,
            } in rows
            {
                let agent = super::decode_agent(&raw)?;
                if agent.id != id {
                    return Err(Error::database_error("Invalid purge agent identity."));
                }
                response.selected_agents = response
                    .selected_agents
                    .checked_add(1)
                    .ok_or_else(super::numeric_range)?;
                if response.selected_preview.len() < PREVIEW {
                    response.selected_preview.push(id.clone());
                }
                if let Some(reason) =
                    self.purge_blocker(&agent, claimed || running.contains(&id), blocked)?
                {
                    response.blocked_agents = response
                        .blocked_agents
                        .checked_add(1)
                        .ok_or_else(super::numeric_range)?;
                    if response.blockers.len() < PREVIEW {
                        response.blockers.push(api::PurgeBlocker {
                            agent_id: id.clone(),
                            reason,
                        });
                    }
                }
                after = id;
            }
        }
        Ok(())
    }
}
