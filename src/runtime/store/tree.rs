//! Compact agent-tree rows and attach helpers for human viewers.
mod queries;
use super::{Queries, numeric};
use crate::{
    error::Result,
    runtime::{api, domain::Agent},
};
use rusqlite::params;

use queries::{COUNTS, LIVE_AGENTS, LIVE_ROOTS, SUBTREE, TAIL_AFTER};

/// Mailbox and child counts that the public agent record does not carry.
pub(crate) struct Counts {
    pub queued: u64,
    pub mail: u64,
    pub children: u64,
}

impl Queries {
    pub(crate) fn counts(&self, id: &str) -> Result<Counts> {
        let (queued, mail, children): (i64, i64, i64) =
            self.query_row_cached(COUNTS, [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(Counts {
            queued: u64::try_from(queued).map_err(numeric)?,
            mail: u64::try_from(mail).map_err(numeric)?,
            children: u64::try_from(children).map_err(numeric)?,
        })
    }

    /// Agent IDs of a subtree (root first) or of the live roots; `limit + 1`
    /// rows are read so the caller can report truncation.
    /// Every live agent, `limit + 1` rows at most so the caller reports truncation.
    pub(crate) fn live_ids(&self, limit: usize) -> Result<Vec<String>> {
        let rows = i64::try_from(limit.saturating_add(1)).map_err(numeric)?;
        self.sql().cached().query_column(LIVE_AGENTS, [rows])
    }

    pub(crate) fn tree_ids(&self, root: Option<&str>, limit: usize) -> Result<Vec<String>> {
        let rows = i64::try_from(limit.saturating_add(1)).map_err(numeric)?;
        match root {
            Some(root) => {
                self.agent(root)?;
                self.sql()
                    .cached()
                    .query_column(SUBTREE, params![root, rows])
            }
            None => self.sql().cached().query_column(LIVE_ROOTS, [rows]),
        }
    }

    pub(crate) fn tree_row(&self, agent: &Agent) -> Result<api::TreeRow> {
        let Counts {
            queued,
            mail,
            children,
        } = self.counts(&agent.id)?;
        Ok(api::TreeRow {
            id: agent.id.clone(),
            parent_id: agent.parent_id.clone(),
            role: agent.role.clone(),
            name: agent.name.clone(),
            state: agent.state,
            task_outcome: agent.task_outcome,
            waiting_reason: agent.waiting_reason,
            auth: agent.auth.clone(),
            auth_group: agent.auth_group.clone(),
            auth_decision: agent.auth_decision,
            model: agent.model.clone(),
            effort: agent.effort.clone(),
            sandbox: agent.sandbox,
            ended_at: agent.ended_at,
            children,
            queued,
            mail,
            approval_id: self.pending_approval(&agent.id)?,
        })
    }

    /// The `read` cursor that returns an agent's newest `limit` events.
    pub(crate) fn tail_after(&self, id: &str, limit: u64) -> Result<u64> {
        let offset = i64::try_from(limit).map_err(numeric)?;
        let seq: Option<i64> =
            self.query_opt_cached(TAIL_AFTER, params![id, offset], |r| r.get(0))?;
        seq.map_or(Ok(0), |seq| u64::try_from(seq).map_err(numeric))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Fixtures stop immediately on store setup failure.
mod tests;
