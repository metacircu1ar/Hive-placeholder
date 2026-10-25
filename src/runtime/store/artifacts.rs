//! Read artifact byte ranges and complete committed artifact deletions.
mod deletion;
mod queries;
use super::{Queries, Store, api, queries as shared_queries, rows};
use crate::{
    error::{Error, Result},
    state,
};
pub(in crate::runtime) use deletion::unlink_artifacts;
use serde_json::Value;
use std::{
    io::{Seek, SeekFrom},
    path::PathBuf,
};
// Commit artifact unlink progress after each bounded page of pending deletions.
const PAGE: usize = 128;
impl Store {
    /// `encoded_len` is the payload's JSON byte length, already measured by the caller.
    pub(super) fn publish_artifact(
        &self,
        data: &Value,
        encoded_len: usize,
        agent: Option<&str>,
        join: Option<&str>,
    ) -> Result<(String, usize)> {
        let id = uuid::Uuid::new_v4().to_string();
        let bytes = encoded_len
            .checked_add(1)
            .ok_or_else(super::numeric_range)?;
        let stored_bytes = i64::try_from(bytes).map_err(super::numeric)?;
        let created_at = super::convert::now()?;
        // Durable publication precedes the SQL reference. Rollback can leave an orphan,
        // which compaction removes, but never a dangling committed reference.
        state::write_json(&self.artifacts_dir.join(format!("{id}.json")), data)?;
        self.execute_cached(
            shared_queries::INSERT_ARTIFACT,
            rusqlite::params![id, stored_bytes, created_at, agent, join],
        )?;
        Ok((id, bytes))
    }
}
pub(in crate::runtime) struct ArtifactDeletions {
    pub dir: PathBuf,
    pub ids: Vec<String>,
}

pub(super) fn missing_file(error: &Error) -> bool {
    error.io_kind() == Some(std::io::ErrorKind::NotFound)
}
impl Queries {
    #[expect(
        clippy::expect_used,
        reason = "Artifact bytes are UTF-8 and truncate only at a validated character boundary."
    )]
    pub fn output(&self, id: &str, offset: u64, limit: usize) -> Result<Value> {
        let limit = api::pagination::bytes(u64::try_from(limit).map_err(super::numeric)?)?;
        let id = uuid::Uuid::parse_str(id)
            .map_err(|cause| Error::invalid_artifact("Invalid artifact ID.").with_source(cause))?
            .to_string();
        let bytes = self
            .sql()
            .query_opt(shared_queries::ARTIFACT_BYTES, [&id], |r| {
                rows::unsigned(r, 0)
            })?
            .ok_or(Error::unknown_artifact("No artifact has this ID."))?;
        #[cfg(test)]
        self.test_hooks.output();
        let mut file = state::private_file(&self.artifacts_dir.join(format!("{id}.json")), false)
            .map_err(|error| {
            if missing_file(&error) {
                Error::artifact_unavailable("Artifact file is no longer available.")
                    .with_source(error)
            } else {
                error
            }
        })?;
        if file.metadata().map_err(Error::state_io_from)?.len() != bytes || offset > bytes {
            return Err(Error::artifact_unavailable(
                "Artifact length or requested offset is invalid.",
            ));
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(Error::state_io_from)?;
        let mut data = crate::os::read_capped(file, limit).map_err(Error::state_io_from)?;
        if let Err(error) = std::str::from_utf8(&data) {
            if error.error_len().is_some() {
                return Err(Error::invalid_artifact_offset(
                    "Offset must start at a UTF-8 boundary.",
                )
                .with_source(error));
            }
            data.truncate(error.valid_up_to());
        }
        let next = offset
            .checked_add(u64::try_from(data.len()).map_err(super::numeric)?)
            .ok_or_else(super::numeric_range)?;
        Ok(api::value(api::OutputResponse {
            artifact_id: id,
            offset,
            text: String::from_utf8(data)
                .expect("artifact bytes were validated and truncated at a UTF-8 boundary"),
            next_offset: next,
            bytes,
            eof: next == bytes,
        }))
    }
    pub(in crate::runtime) fn pending_artifact_deletions(&self) -> Result<ArtifactDeletions> {
        let rows: Vec<super::rows::ArtifactDeletion> = self
            .sql()
            .cached()
            .query_rows(queries::PENDING_ARTIFACT_DELETIONS, [])?;
        if rows.iter().any(|row| row.indexed) {
            return Err(Error::database_error(
                "Artifact deletion conflicts with retained output.",
            ));
        }
        debug_assert!(rows.len() <= PAGE);
        Ok(ArtifactDeletions {
            dir: self.artifacts_dir.clone(),
            ids: rows.into_iter().map(|row| row.id).collect(),
        })
    }
}

impl Store {
    pub(in crate::runtime) fn acknowledge_artifact_deletions(&self, ids: &[String]) -> Result<()> {
        for id in ids {
            self.execute_cached(queries::ACKNOWLEDGE_ARTIFACT_DELETIONS, [id])?;
        }
        Ok(())
    }
    /// Startup/offline maintenance has no concurrent control readers.
    pub(in crate::runtime) fn finish_artifact_deletions(&self) -> Result<()> {
        loop {
            let ArtifactDeletions { dir, ids } = self.pending_artifact_deletions()?;
            if ids.is_empty() {
                return Ok(());
            }
            unlink_artifacts(&dir, &ids)?;
            self.atomic(|store| store.acknowledge_artifact_deletions(&ids))?;
        }
    }
}
