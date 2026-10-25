//! Offline retention owns the same daemon lock as serving. Coordination and
//! delivery records are never pruned along with diagnostic history.
mod manifest;
mod preview;
mod queries;
use super::{Queries, Store, queries as shared_queries, rows};
use crate::{
    error::{Error, Result},
    state,
};
pub use preview::{CompactPreview, compact_preview};
use rusqlite::params;
use std::{fs, path::Path};

impl Store {
    #[expect(
        clippy::expect_used,
        reason = "The empty-batch branch exits before the final-row cursor is read."
    )]
    fn prune(&self, before: i64, retained: i64) -> Result<u64> {
        self.sql()
            .map_error(database)
            .execute_batch(shared_queries::CREATE_COMPACTION_FRONTIER)?;
        self.sql().map_error(database).execute(
            &queries::FILL_COMPACTION_FRONTIER,
            params![retained, before],
        )?;
        let mut after = 0_i64;
        let mut removed = 0_u64;
        loop {
            let rows: Vec<rows::PrunableEvent> = self
                .sql()
                .map_error(database)
                .query_rows(shared_queries::PRUNABLE_EVENTS, params![after, before])?;
            if rows.is_empty() {
                break;
            }
            self.atomic(|store| {
                for row in &rows {
                    store.sql().map_error(database).execute(
                        shared_queries::ADVANCE_EVENT_RETENTION,
                        params![row.agent, row.sequence],
                    )?;
                    store
                        .sql()
                        .map_error(database)
                        .execute(shared_queries::DELETE_EVENT, [&row.sequence])?;
                }
                Ok(())
            })?;
            after = rows
                .last()
                .expect("nonempty pruning batch has a final row")
                .sequence;
            removed = removed
                .checked_add(u64::try_from(rows.len()).map_err(super::numeric)?)
                .ok_or_else(super::numeric_range)?;
        }
        let timer = crate::observability::timing::Timer::start();
        let result = self
            .sql()
            .map_error(database)
            .execute_batch(shared_queries::FINISH_COMPACTION);
        timer.finish("finish_compaction", result.is_ok());
        result?;
        Ok(removed)
    }
    fn prune_calls(&self, before: i64) -> Result<usize> {
        self.atomic(|store| {
            store
                .sql()
                .map_error(database)
                .execute(&queries::PRUNE_CALLS, [before])?;
            store
                .sql()
                .map_error(database)
                .execute(queries::PRUNE_CALLS_2, [])
        })
    }
}
pub(super) fn database(cause: rusqlite::Error) -> Error {
    Error::database_error("Cannot compact runtime storage.").with_source(cause)
}

#[derive(Debug, serde::Serialize)]
pub struct CompactResult {
    pub call_results_removed: usize,
    pub events_removed: u64,
    pub orphan_artifacts_removed: u64,
    pub database_bytes_before: u64,
    pub database_bytes_after: u64,
    pub output_manifest: std::path::PathBuf,
    pub retained: &'static str,
}

pub fn compact(root: &Path, before: u64, retained: u64) -> Result<CompactResult> {
    let (before, retained) = bounds(before, retained)?;
    let _lock = lock(root)?;
    let runtime = root.join("runtime");
    let store = Store::open(&runtime.join("runtime.sqlite"))?;
    store.finish_artifact_deletions()?;
    store.recover_attachments()?;
    let size_before = fs::metadata(runtime.join("runtime.sqlite"))
        .map_err(Error::state_io_from)?
        .len();
    let call_results_removed = store.prune_calls(before)?;
    let removed = store.prune(before, retained)?;
    // Referenced artifacts remain available after event pruning. Only files
    // from rolled-back publications, absent from the artifact index, are orphans.
    let orphans = visit_orphans(&store, |path| {
        fs::remove_file(path).map_err(Error::state_io_from)
    })?;
    fs::File::open(&store.artifacts_dir)
        .and_then(|f| f.sync_all())
        .map_err(Error::state_io_from)?;
    store.manifest(&runtime)?;
    store.checkpoint()?;
    let size_after = fs::metadata(runtime.join("runtime.sqlite"))
        .map_err(Error::state_io_from)?
        .len();
    tracing::info!(name: "compact.completed", count = removed, bytes = size_after);
    Ok(CompactResult {
        events_removed: removed,
        call_results_removed,
        orphan_artifacts_removed: orphans,
        database_bytes_before: size_before,
        database_bytes_after: size_after,
        output_manifest: runtime.join("output-manifest.jsonl"),
        retained: "Agent identities, messages, delivery provenance, request replay records, \
            provider evidence and indexed artifacts",
    })
}

fn lock(root: &Path) -> Result<state::Lock> {
    let state = state::State::new(root.to_owned())?;
    let admission = state.admission()?;
    state.check_format()?;
    state::check_path(root, true)?;
    let runtime = root.join("runtime");
    state::check_path(&runtime, true)?;
    Ok(state::Lock::acquire(&runtime.join("daemon.lock"))?.with_admission(admission))
}

fn bounds(before: u64, retained: u64) -> Result<(i64, i64)> {
    for (value, range, field) in [
        (before, crate::limits::COMPACT_BEFORE, "--before"),
        (retained, crate::limits::COMPACT_KEEP, "--keep-per-agent"),
    ] {
        hive_input::validate::integer(value, range).map_err(|rule| {
            hive_input::InputError::new(
                rule.code(),
                "Invalid compaction argument.",
                field,
                rule.expected(),
                vec!["hivectl compact --before 1700000000 --keep-per-agent 100 --dry-run".into()],
            )
            .map_or_else(
                |cause| Error::internal_error("Invalid input-error metadata.").with_source(cause),
                Error::from,
            )
        })?;
    }
    Ok((
        i64::try_from(before).map_err(super::numeric)?,
        i64::try_from(retained).map_err(super::numeric)?,
    ))
}

fn visit_orphans(queries: &Queries, mut visit: impl FnMut(&Path) -> Result<()>) -> Result<u64> {
    let mut orphans = 0_u64;
    for entry in fs::read_dir(&queries.artifacts_dir).map_err(Error::state_io_from)? {
        let entry = entry.map_err(Error::state_io_from)?;
        let path = entry.path();
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.extension().is_none_or(|s| s != "json") || uuid::Uuid::parse_str(id).is_err() {
            continue;
        }
        let (present, pending): (bool, bool) =
            queries
                .sql()
                .map_error(database)
                .query_row(queries::OUTPUT_OWNERSHIP, [id], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?;
        if !present && !pending {
            state::check_path(&path, false)?;
            visit(&path)?;
            orphans = orphans.checked_add(1).ok_or_else(super::numeric_range)?;
        }
    }
    Ok(orphans)
}
