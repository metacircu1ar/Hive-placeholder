//! Message-owned snapshot references and a committed physical-deletion outbox.
mod queries;
use super::{Queries, Store};
use crate::{
    error::{Error, Result},
    runtime::attachments::{self, Attachment},
    state,
};
use rusqlite::params;
use std::{fs, path::PathBuf};

pub(in crate::runtime) struct Deletion {
    pub key: String,
    pub filename: String,
}
impl Queries {
    pub(in crate::runtime) fn latest_attachments(
        &self,
        agent: &str,
    ) -> Result<Option<crate::runtime::api::AttachedMessage>> {
        let id: Option<String> =
            self.query_opt_cached(queries::LATEST_ATTACHMENTS_SELECT, [agent], |r| r.get(0))?;
        id.map(|message_id| {
            Ok(crate::runtime::api::AttachedMessage {
                attachments: self.attachments(&message_id)?,
                message_id,
            })
        })
        .transpose()
    }

    pub(in crate::runtime) fn unindexed_attachments(
        &self,
        files: Vec<attachments::Snapshot>,
    ) -> Result<Vec<attachments::Snapshot>> {
        files
            .into_iter()
            .map(|file| {
                let indexed: bool = self.query_row_cached(
                    queries::UNINDEXED_ATTACHMENTS_SELECT,
                    [&file.key],
                    |r| r.get(0),
                )?;
                Ok((!indexed).then_some(file))
            })
            .filter_map(Result::transpose)
            .collect()
    }
    pub(in crate::runtime) fn attachments(&self, message: &str) -> Result<Vec<Attachment>> {
        let rows = self
            .sql()
            .cached()
            .query_map(queries::ATTACHMENTS_SELECT, [message], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                ))
            })?;
        rows.into_iter()
            .map(|(raw, key, name, bytes)| {
                let item: Attachment = serde_json::from_str(&raw).map_err(|e| {
                    Error::database_error("Invalid attachment metadata.").with_source(e)
                })?;
                item.validate(&self.attachments_dir)?;
                if item.key() != key.as_deref()
                    || key.is_some()
                        && (name.as_deref() != Some(&item.filename)
                            || bytes != Some(crate::convert::capped_i64(item.bytes)))
                {
                    return Err(Error::state_inconsistent(
                        "Attachment metadata contradicts its ownership row.",
                    ));
                }
                Ok(item)
            })
            .collect()
    }

    pub(in crate::runtime) fn pending_attachments(&self) -> Result<(PathBuf, Vec<Deletion>)> {
        let rows = self
            .sql()
            .cached()
            .query_map(queries::PENDING_ATTACHMENTS_SELECT, [], |r| {
                Ok(Deletion {
                    key: r.get(0)?,
                    filename: r.get(1)?,
                })
            })?;
        Ok((self.attachments_dir.clone(), rows))
    }
    pub(in crate::runtime) fn attachment_bytes(&self) -> Result<u64> {
        self.query_row_cached(queries::ATTACHMENT_BYTES_SELECT, [], |r| {
            super::rows::unsigned(r, 0)
        })
    }
}
impl Store {
    pub(in crate::runtime) fn attach(&self, message: &str, items: &[Attachment]) -> Result<()> {
        for (position, item) in items.iter().enumerate() {
            if let Some(key) = item.key() {
                self.execute_cached(
                    queries::ATTACH_INSERT,
                    params![key, item.filename, crate::convert::capped_i64(item.bytes)],
                )?;
                let matches: bool = self.query_row_cached(
                    queries::ATTACH_SELECT,
                    params![key, item.filename, crate::convert::capped_i64(item.bytes)],
                    |r| r.get(0),
                )?;
                if !matches {
                    return Err(Error::state_inconsistent(
                        "Attachment key metadata differs.",
                    ));
                }
            }
            let data = serde_json::to_string(item).map_err(|e| {
                Error::database_error("Cannot encode attachment metadata.").with_source(e)
            })?;
            self.execute_cached(
                queries::ATTACH_INSERT_2,
                params![
                    message,
                    crate::convert::capped_i64(position as u64),
                    item.key(),
                    data
                ],
            )?;
        }
        Ok(())
    }
    pub(in crate::runtime) fn acknowledge_attachments(&self, removed: &[Deletion]) -> Result<()> {
        for item in removed {
            let referenced: bool =
                self.query_row_cached(queries::ACKNOWLEDGE_ATTACHMENTS_SELECT, [&item.key], |r| {
                    r.get(0)
                })?;
            if referenced {
                return Err(Error::state_inconsistent(
                    "Attachment deletion raced a live reference.",
                ));
            }
            self.execute_cached(queries::ACKNOWLEDGE_ATTACHMENTS_DELETE, [&item.key])?;
            self.execute_cached(queries::ACKNOWLEDGE_ATTACHMENTS_DELETE_2, [&item.key])?;
        }
        Ok(())
    }
    /// Called only under exclusive daemon ownership before admission, or offline compaction.
    pub(in crate::runtime) fn recover_attachments(&self) -> Result<()> {
        loop {
            let (root, pending) = self.pending_attachments()?;
            if pending.is_empty() {
                break;
            }
            for item in &pending {
                attachments::remove_snapshot(&root, &item.key, &item.filename)?;
            }
            self.atomic(|s| s.acknowledge_attachments(&pending))?;
        }
        attachments::sweep_staging(&self.attachments_dir)?;
        for entry in fs::read_dir(&self.attachments_dir).map_err(Error::state_io_from)? {
            let entry = entry.map_err(Error::state_io_from)?;
            let key = entry
                .file_name()
                .into_string()
                .map_err(|_| Error::unsafe_state("Invalid attachment directory name."))?;
            if !attachments::valid_key(&key) {
                return Err(Error::unsafe_state("Unknown attachment store entry."));
            }
            state::check_path(&entry.path(), true)?;
            let indexed: bool =
                self.query_row_cached(queries::RECOVER_ATTACHMENTS_SELECT, [&key], |r| r.get(0))?;
            if !indexed {
                attachments::remove_orphan(&self.attachments_dir, &key)?;
            }
        }
        Ok(())
    }
}
