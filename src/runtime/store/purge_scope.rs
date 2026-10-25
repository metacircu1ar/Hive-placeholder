//! Build purge frontiers and delete the same owned rows for purge and garbage collection.
mod queries;
use super::{Queries, Store, api};
use crate::error::{Error, Result};

impl Queries {
    pub(super) fn purge_counts(&self) -> Result<api::PurgeCounts> {
        let mut counts = api::PurgeCounts::default();
        for query in super::queries::PURGE_COUNTS.iter() {
            *(query.destination)(&mut counts) = self
                .sql()
                .query_row(&query.sql, [], |row| super::rows::count(row, 0))?;
        }
        counts.attachment_bytes = self.query_row_cached(queries::PURGE_COUNTS_SELECT, [], |r| {
            super::rows::unsigned(r, 0)
        })?;
        Ok(counts)
    }
}

impl Store {
    pub(super) fn prepare_purge_scope(&self, target: &str, recursive: bool) -> Result<()> {
        if recursive {
            self.agent_or_purged(target)?;
        } else {
            self.agent(target)?;
        }
        self.create_purge_scope()?;
        if recursive {
            self.sql()
                .execute(queries::FILL_RECURSIVE_SCOPE, [target])?;
            let cycle: bool = self
                .sql()
                .query_row(queries::SCOPE_HAS_CYCLE, [target], |r| r.get(0))?;
            if cycle {
                return Err(Error::database_error("Cyclic agent purge relationship."));
            }
        } else {
            self.sql()
                .execute(super::queries::FILL_SINGLE_SCOPE, [target])?;
        }
        // Tombstones participate in traversal, but never count as agent records to remove.
        let invalid_id: bool = self
            .sql()
            .query_row(queries::SCOPE_HAS_INVALID_ID, [], |r| r.get(0))?;
        if invalid_id {
            return Err(Error::database_error("Invalid purge agent identity."));
        }
        self.sql().execute(queries::REMOVE_SCOPE_TOMBSTONES, [])?;
        self.prepare_purge_snapshots()
    }
    pub(super) fn create_purge_scope(&self) -> Result<()> {
        self.sql().execute_batch(queries::CREATE_PURGE_SCOPE)
    }
    pub(super) fn prepare_purge_snapshots(&self) -> Result<()> {
        self.execute_cached(queries::PREPARE_PURGE_SNAPSHOTS, [])?;
        Ok(())
    }
    pub(super) fn clear_purge_scope(&self) -> Result<()> {
        self.sql().execute_batch(queries::CLEAR_PURGE_SCOPE)
    }
    pub(super) fn delete_purge_scope(&self) -> Result<()> {
        self.sql()
            .execute_batch(&super::queries::DELETE_PURGE_SCOPE)?;
        Ok(())
    }
}
