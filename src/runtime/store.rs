//! Runtime storage facade, request replay and writer wake bookkeeping.
use super::api;
use super::domain::{Agent, EventKind};
use crate::error::{Error, Result};
use rusqlite::params;
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
};

mod agents;
mod approvals;
mod artifacts;
mod attachment_images;
mod attachments;
mod auth_fallback;
mod auth_load;
mod branching;
mod calls;
mod checkpointer;
pub(in crate::runtime) use checkpointer::Checkpointer;
#[cfg(feature = "comb")]
mod collectors;
mod convert;
mod events;
mod features;
mod files;
mod gc;
mod handoff;
mod joins;
mod maintenance;
mod messages;
mod migration;
pub(in crate::runtime) mod wal;
pub(crate) use migration::inspect as inspect_migration_database;
mod notices;
mod owned;
mod process_options;
mod purge;
pub(super) use purge::{ArtifactDeletions, unlink_artifacts};
mod purge_scope;
mod queries;
mod redirect;
mod rows;
use convert::{numeric, numeric_range};
mod sql;
use sql::FromRow as _;
pub(super) use sql::sqlite_code;
mod scheduled;
use agents::decode_agent;
pub use files::Queries;
pub(crate) use files::check_files as check_existing_paths;
#[cfg(test)]
pub(crate) use files::read_connection_count;
pub(crate) use files::{check_existing_format, check_existing_identity, database_admission};
pub use messages::{Message, TurnReservation};
pub(super) use scheduled::ScheduledProof;
mod schema;
mod stalls;
mod steering;
#[cfg(test)]
#[allow(clippy::unwrap_used)] // Test fixtures and assertions deliberately panic on failure.
mod tests;
mod tree;

pub use maintenance::{CompactPreview, CompactResult, compact, compact_preview};
#[cfg(test)]
pub(crate) use schema::initialize as initialize_schema;
pub(crate) use schema::validate as validate_schema;

pub(super) struct WakeChanges {
    pub agents: HashSet<String>,
    pub all: bool,
    pub deadlines: bool,
    pub observed: Observed,
}

/// Live-viewer signals from one commit; never worker wakes.
pub(super) struct Observed {
    /// Agents whose durable history changed.
    pub agents: HashSet<String>,
    pub all: bool,
    /// Viewer-visible rows changed outside an agent save (approvals).
    pub tree: bool,
}

pub(super) struct WriterCounters {
    pub memory: crate::os::SqliteMemory,
    pub pending_wakes: usize,
}
pub struct Store {
    queries: Queries,
    transaction_depth: Cell<usize>,
    notices: RefCell<notices::Notices>,
    pending_wakes: RefCell<HashSet<String>>,
    wake_all: Cell<bool>,
    deadlines_changed: Cell<bool>,
    // Live-viewer signals only; never worker wakes.
    observed: RefCell<HashSet<String>>,
    observed_all: Cell<bool>,
    tree_changed: Cell<bool>,
    // Conservative across savepoint rollback: once true, never cleared while open.
    joins_possible: Cell<bool>,
    #[cfg(feature = "comb")]
    collectors_possible: Cell<bool>,
}
// Covers the steady-state statement set, including nested savepoints, without row caching.
const STATEMENT_CACHE_CAPACITY: usize = 64;
// Switch to waking every agent when individual wake IDs reach this limit.
const PENDING_WAKE_LIMIT: usize = 256;
// Stop a page before this serialized-byte budget is exceeded, unless it is the first item.
const PAGE_BYTES: usize = 768 * 1024;
// Bound each child traversal page before another database round trip.
pub(super) const CHILD_PAGE_ROWS: usize = 128;
/// Byte-bounded pages always admit their first item, including an oversized one.
struct PageBudget {
    bytes: usize,
    items: usize,
}
impl PageBudget {
    fn new() -> Self {
        Self { bytes: 0, items: 0 }
    }
    fn accept(&mut self, size: usize) -> Result<bool> {
        let next = self.bytes.checked_add(size).ok_or_else(numeric_range)?;
        if next > PAGE_BYTES && self.items != 0 {
            return Ok(false);
        }
        self.bytes = next;
        self.items += 1;
        Ok(true)
    }
}
impl Queries {
    pub fn prior(&self, sender: &str, id: &str, body: &str) -> Result<Option<Value>> {
        self.prior_protocol(sender, id, body, crate::compatibility::CONTROL_PROTOCOL)
    }
    pub fn prior_protocol(
        &self,
        sender: &str,
        id: &str,
        body: &str,
        protocol: u32,
    ) -> Result<Option<Value>> {
        let old: Option<rows::RequestReplay> = self.query_opt_cached(
            queries::PRIOR_REQUEST,
            params![sender, id],
            rows::RequestReplay::read,
        )?;
        match old {
            None => Ok(None),
            Some(replay) if replay.body == body && replay.protocol != protocol => {
                #[derive(serde::Serialize)]
                struct Unavailable<'a> {
                    error: Error,
                    request_id: &'a str,
                }
                Ok(Some(api::value(Unavailable {
                    error: Error::replay_unavailable(
                        "The original request was answered in another protocol; it was not \
                            executed again. Use its original protocol to retrieve the reply.",
                    ),
                    request_id: id,
                })))
            }
            Some(replay) if replay.body == body => {
                let response: Value = serde_json::from_str(&replay.response).map_err(|cause| {
                    Error::database_error("Invalid saved request response.").with_source(cause)
                })?;
                Self::validate_replay(&response)?;
                let encoded = response.to_string();
                if encoded != replay.response {
                    return Err(Error::database_error(
                        "Saved request response is not its original canonical encoding.",
                    ));
                }
                Ok(Some(response))
            }
            Some(_) => Err(Error::request_conflict(
                "Request ID was already used with different arguments.",
            )),
        }
    }
}

impl Store {
    /// The writer checkpoints after its replies instead of inside COMMIT.
    pub(super) fn defer_checkpoints(&self) -> Result<()> {
        use sql::RuntimeResult as _;
        wal::install(&self.connection).runtime()
    }
    /// A checkpointing connection to the same database, on its own thread.
    pub(super) fn start_checkpointer(&self) -> Result<checkpointer::Checkpointer> {
        let path = self
            .connection
            .path()
            .filter(|path| !path.is_empty())
            .ok_or_else(|| Error::database_error("The runtime database has no file path."))?;
        checkpointer::Checkpointer::start(std::path::Path::new(path))
    }

    pub fn changes(&self) -> WakeChanges {
        WakeChanges {
            agents: self.pending_wakes.take(),
            all: self.wake_all.replace(false),
            deadlines: self.deadlines_changed.replace(false),
            observed: Observed {
                agents: self.observed.take(),
                all: self.observed_all.replace(false),
                tree: self.tree_changed.replace(false),
            },
        }
    }
    /// Durable history changed for a viewer; workers are not woken.
    fn observed(&self, id: &str) {
        if self.observed.borrow().len() < PENDING_WAKE_LIMIT {
            self.observed.borrow_mut().insert(id.to_owned());
        } else {
            self.observed_all.set(true);
        }
    }
    pub(super) fn deadlines_changed(&self) {
        self.deadlines_changed.set(true);
    }
    fn changed(&self, id: &str) {
        if self.pending_wakes.borrow().len() < PENDING_WAKE_LIMIT {
            self.pending_wakes.borrow_mut().insert(id.to_owned());
        } else {
            self.wake_all.set(true);
        }
    }

    pub(super) fn writer_counters(&self) -> WriterCounters {
        WriterCounters {
            memory: crate::os::sqlite_memory(&self.connection),
            pending_wakes: self.pending_wakes.borrow().len(),
        }
    }
    pub(super) fn total_changes(&self) -> u64 {
        self.connection.total_changes()
    }

    pub fn remember(&self, sender: &str, id: &str, body: &str, result: &Value) -> Result<()> {
        self.execute_cached(
            queries::REMEMBER_REQUEST,
            params![
                sender,
                id,
                body,
                result.to_string(),
                crate::compatibility::CONTROL_PROTOCOL
            ],
        )?;
        Ok(())
    }
}

impl std::ops::Deref for Store {
    type Target = Queries;
    fn deref(&self) -> &Self::Target {
        &self.queries
    }
}
