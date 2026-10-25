//! Read-only eligibility counts; recovery remains an execution-time operation.
use super::{Queries, bounds, database, lock, queries, rows, visit_orphans};
use crate::error::Result;
use rusqlite::params;
use std::path::Path;

#[derive(Debug, serde::Serialize)]
pub struct CompactPreview {
    pub dry_run: bool,
    pub events_to_remove: u64,
    pub call_results_to_remove: usize,
    pub orphan_artifacts_to_remove: u64,
    pub pending_artifact_deletions: u64,
    pub attachment_recovery: &'static str,
}

pub fn compact_preview(root: &Path, before: u64, retained: u64) -> Result<CompactPreview> {
    let (before, retained) = bounds(before, retained)?;
    let _lock = lock(root)?;
    let reader = Queries::open_read_only(&root.join("runtime/runtime.sqlite"))?;
    reader.snapshot(|reader| {
        Ok(CompactPreview {
            dry_run: true,
            events_to_remove: reader.sql().map_error(database).query_row(
                &queries::PREVIEW_EVENTS,
                params![retained, before],
                |row| rows::unsigned(row, 0),
            )?,
            call_results_to_remove: reader.sql().map_error(database).query_row(
                &queries::PREVIEW_CALLS,
                [before],
                |row| rows::count(row, 0),
            )?,
            orphan_artifacts_to_remove: visit_orphans(reader, |_| Ok(()))?,
            pending_artifact_deletions: reader.sql().map_error(database).query_row(
                queries::PENDING_ARTIFACTS,
                [],
                |row| rows::unsigned(row, 0),
            )?,
            attachment_recovery: "not_evaluated",
        })
    })
}
