//! Indexed retention selection; the caller owns one transaction per page.
mod queries;
use super::convert::timestamp as unsigned;
use super::{Agent, Queries, Store, api};
use crate::error::{Error, Result};
use rusqlite::params;
use std::collections::HashSet;

use crate::runtime::limits::GC_CANDIDATES as CANDIDATES;
// A GC page stops before exceeding this aggregate owned-row deletion budget.
const ROW_BUDGET: u64 = 100_000;

// Tuple comparison seeks into the partial index rather than rescanning past pages.

impl Queries {
    fn gc_candidates(
        &self,
        before: u64,
        after: Option<&api::GcCursor>,
    ) -> Result<Vec<api::GcCursor>> {
        self.sql().cached().query_map(
            queries::CANDIDATE_PAGE,
            params![
                crate::convert::capped_i64(before),
                after.map_or(-1, |c| crate::convert::capped_i64(c.ended_at)),
                after.map_or("", |c| c.id.as_str())
            ],
            |r| {
                Ok(api::GcCursor {
                    id: r.get(0)?,
                    ended_at: unsigned(r, 1)?,
                })
            },
        )
    }
    fn gc_blocker(
        &self,
        agent: &Agent,
        running: &HashSet<String>,
    ) -> Result<Option<crate::runtime::domain::PurgeBlockerReason>> {
        let (claimed, blocked) = self.query_row_cached(queries::GC_BLOCKER, [&agent.id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
        self.purge_blocker(agent, claimed || running.contains(&agent.id), blocked)
    }
    fn gc_owned_rows(&self, id: &str) -> Result<u64> {
        self.query_row_cached(&super::queries::GC_OWNED_ROWS, [id], |r| unsigned(r, 0))
    }
}

impl Store {
    pub(in crate::runtime) fn gc(
        &self,
        args: &api::GcArgs,
        running: &HashSet<String>,
    ) -> Result<api::GcPage> {
        let api::GcOptions {
            before,
            after,
            dry_run,
        } = args.options()?;
        let mut page = api::GcPage {
            request_id: None,
            before,
            dry_run,
            stats: api::GcStats::default(),
            selected_preview: vec![],
            blockers: vec![],
            next_after: None,
            cleanup_pending: false,
        };
        let mut cursor = after.cloned().unwrap_or(api::GcCursor {
            ended_at: 0,
            id: String::new(),
        });
        let rows = self.gc_candidates(before, after)?;
        debug_assert!(rows.len() <= CANDIDATES);
        self.create_purge_scope()?;
        for row in rows {
            if !self.select_gc_candidate(row, running, &mut page, &mut cursor)? {
                break;
            }
        }
        self.finish_gc_page(page, cursor)
    }
    fn finish_gc_page(&self, mut page: api::GcPage, cursor: api::GcCursor) -> Result<api::GcPage> {
        self.prepare_purge_snapshots()?;
        let counts = self.purge_counts()?;
        page.stats.messages = counts.messages as u64;
        page.stats.attachments = counts.attachments as u64;
        page.stats.attachment_bytes = counts.attachment_bytes;
        page.stats.approvals = counts.approvals as u64;
        page.stats.events = counts.events as u64;
        page.stats.artifacts = counts.artifacts as u64;
        page.stats.indexed_output_bytes =
            self.query_row_cached(queries::SCOPE_ARTIFACT_BYTES, [], |r| unsigned(r, 0))?;
        if !page.dry_run {
            self.delete_purge_scope()?;
            page.stats.deleted_agents = page.stats.eligible_agents;
        }
        self.clear_purge_scope()?;
        let more: bool = self.query_row_cached(
            queries::HAS_MORE,
            params![
                crate::convert::capped_i64(page.before),
                crate::convert::capped_i64(cursor.ended_at),
                cursor.id
            ],
            |r| r.get(0),
        )?;
        if more {
            page.next_after = Some(cursor);
        }
        if !page.dry_run {
            self.maintenance_notice(true, page.stats.deleted_agents);
        }
        Ok(page)
    }
    fn select_gc_candidate(
        &self,
        row: api::GcCursor,
        running: &HashSet<String>,
        page: &mut api::GcPage,
        cursor: &mut api::GcCursor,
    ) -> Result<bool> {
        let agent = self.agent(&row.id)?;
        if row.id.is_empty()
            || agent.id != row.id
            || agent.ended_at != Some(row.ended_at)
            || !agent.terminal()
        {
            return Err(Error::database_error("Invalid GC candidate record."));
        }
        let reason = self.gc_blocker(&agent, running)?;
        let work = if reason.is_none() {
            self.gc_owned_rows(&row.id)?
        } else {
            0
        };
        if page.stats.eligible_agents > 0 && work > ROW_BUDGET.saturating_sub(page.stats.work_rows)
        {
            return Ok(false);
        }
        *cursor = row;
        page.stats.examined_agents += 1;
        if let Some(reason) = reason {
            page.stats.blocked_agents += 1;
            if page.blockers.len() < api::GC_PREVIEW {
                page.blockers.push(api::PurgeBlocker {
                    agent_id: cursor.id.clone(),
                    reason,
                });
            }
            return Ok(true);
        }
        page.stats.eligible_agents += 1;
        page.stats.work_rows += work;
        page.stats.largest_agent_rows = page.stats.largest_agent_rows.max(work);
        self.execute_cached(super::queries::FILL_SINGLE_SCOPE, [&cursor.id])?;
        if page.selected_preview.len() < api::GC_PREVIEW {
            page.selected_preview.push(cursor.id.clone());
        }
        Ok(page.stats.work_rows < ROW_BUDGET)
    }
}

#[cfg(test)]
mod tests;
