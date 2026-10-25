//! SQLite execution helpers preserve statement caching and domain error context.
use crate::error::{Error, Result};
use rusqlite::{Connection, OptionalExtension, Params, Row, Statement, types::FromSql};

pub(super) trait FromRow: Sized {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self>;
}

pub(super) fn db(cause: rusqlite::Error) -> Error {
    Error::database_error("Cannot access the runtime database.").with_source(cause)
}

/// Connection setup also uses the runtime database error classification.
pub(super) trait RuntimeResult<T> {
    fn runtime(self) -> Result<T>;
}
impl<T> RuntimeResult<T> for rusqlite::Result<T> {
    fn runtime(self) -> Result<T> {
        self.map_err(db)
    }
}

#[derive(Clone, Copy)]
pub(super) struct Database<'a> {
    connection: &'a Connection,
    error: fn(rusqlite::Error) -> Error,
    cached: bool,
    poison: Option<&'a std::cell::RefCell<Option<Error>>>,
}
impl<'a> Database<'a> {
    pub fn new(connection: &'a Connection) -> Self {
        Self {
            connection,
            error: db,
            cached: false,
            poison: None,
        }
    }
    fn healthy(self) -> Result<()> {
        match self.poison.and_then(|poison| poison.borrow().clone()) {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    pub fn map_error(self, error: fn(rusqlite::Error) -> Error) -> Self {
        Self { error, ..self }
    }
    pub fn cached(self) -> Self {
        Self {
            cached: true,
            ..self
        }
    }
    pub fn execute(self, sql: &str, params: impl Params) -> Result<usize> {
        self.statement(sql, |statement| {
            statement.execute(params).map_err(self.error)
        })
    }
    pub fn execute_batch(self, sql: &str) -> Result<()> {
        self.healthy()?;
        self.connection.execute_batch(sql).map_err(self.error)
    }
    pub fn query_row<T>(
        self,
        sql: &str,
        params: impl Params,
        row: impl FnOnce(&Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<T> {
        self.statement(sql, |statement| {
            statement.query_row(params, row).map_err(self.error)
        })
    }
    pub fn query_opt<T>(
        self,
        sql: &str,
        params: impl Params,
        row: impl FnOnce(&Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Option<T>> {
        self.statement(sql, |statement| {
            statement
                .query_row(params, row)
                .optional()
                .map_err(self.error)
        })
    }
    pub fn query_rows<T: FromRow>(self, sql: &str, params: impl Params) -> Result<Vec<T>> {
        self.query_map(sql, params, T::read)
    }
    pub fn query_column<T: FromSql>(self, sql: &str, params: impl Params) -> Result<Vec<T>> {
        self.query_map(sql, params, |row| row.get(0))
    }
    pub fn query_map<T>(
        self,
        sql: &str,
        params: impl Params,
        row: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Vec<T>> {
        self.statement(sql, |statement| {
            statement
                .query_map(params, row)
                .and_then(Iterator::collect)
                .map_err(self.error)
        })
    }
    pub fn with_rows<T>(
        self,
        sql: &str,
        params: impl Params,
        read: impl FnOnce(&mut Cursor<'_>) -> Result<T>,
    ) -> Result<T> {
        self.statement(sql, |statement| {
            let rows = statement.query(params).map_err(self.error)?;
            read(&mut Cursor {
                rows,
                error: self.error,
            })
        })
    }
    fn statement<T>(
        self,
        sql: &str,
        read: impl FnOnce(&mut Statement<'_>) -> Result<T>,
    ) -> Result<T> {
        self.healthy()?;
        if self.cached {
            let mut statement = self.connection.prepare_cached(sql).map_err(self.error)?;
            read(&mut statement)
        } else {
            let mut statement = self.connection.prepare(sql).map_err(self.error)?;
            read(&mut statement)
        }
    }
}

/// Reads only the requested rows; page limits must not decode later records.
pub(super) struct Cursor<'a> {
    rows: rusqlite::Rows<'a>,
    error: fn(rusqlite::Error) -> Error,
}
impl Cursor<'_> {
    pub fn next<T: FromRow>(&mut self) -> Result<Option<T>> {
        self.next_with(T::read)
    }
    pub fn column<T: FromSql>(&mut self) -> Result<Option<T>> {
        self.next_with(|row| row.get(0))
    }
    pub fn next_with<T>(
        &mut self,
        row: impl FnOnce(&Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Option<T>> {
        self.rows
            .next()
            .and_then(|value| value.map(row).transpose())
            .map_err(self.error)
    }
}

use super::{Queries, Store, queries};

impl Queries {
    pub(in crate::runtime) fn snapshot<T>(
        &self,
        read: impl FnOnce(&Self) -> Result<T>,
    ) -> Result<T> {
        self.sql().execute_batch(queries::SNAPSHOT)?;
        let result = read(self);
        // ROLLBACK closes a read-only snapshot on every Result path, without writes.
        let closed = self.close_snapshot();
        if let Err(error) = &closed {
            self.poison(error);
            cleanup_failure("snapshot", result.as_ref().err().unwrap_or(error), error);
        }
        match result {
            Ok(value) => closed.map(|()| value),
            Err(error) => Err(error),
        }
    }
    fn close_snapshot(&self) -> Result<()> {
        #[cfg(test)]
        self.test_hooks
            .sql("snapshot_close", &self.connection)
            .runtime()?;
        self.sql().execute_batch(queries::SNAPSHOT_2)
    }
    pub(in crate::runtime) fn healthy(&self) -> Result<()> {
        self.sql().healthy()
    }
    fn poison(&self, error: &Error) {
        let mut poison = self.poison.borrow_mut();
        if poison.is_none() {
            *poison = Some(error.clone());
        }
    }
    pub(in crate::runtime) fn autocommit(&self) -> bool {
        self.connection.is_autocommit()
    }
    pub(super) fn sql(&self) -> super::sql::Database<'_> {
        Database {
            poison: Some(&self.poison),
            ..Database::new(&self.connection)
        }
    }
    pub(super) fn query_opt_cached<T>(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
        row: impl FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Option<T>> {
        self.sql().cached().query_opt(sql, params, row)
    }
    pub(super) fn query_row_cached<T>(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
        row: impl FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<T> {
        self.sql().cached().query_row(sql, params, row)
    }
}

impl Store {
    /// Live writer only: savepoint undo pages need not spill to temporary files.
    /// Call after recovery and outside a transaction. Offline compaction keeps
    /// disk-backed temporaries because its frontier can span the entire history.
    pub(in crate::runtime) fn use_memory_temporaries(&self) -> Result<()> {
        self.healthy()?;
        self.connection
            .pragma_update(None, "temp_store", "MEMORY")
            .runtime()
    }
    pub(super) fn execute_cached(&self, sql: &str, params: impl rusqlite::Params) -> Result<usize> {
        self.sql().cached().execute(sql, params)
    }
    /// Nested savepoints retain the existing wake superset until the writer commits.
    pub(in crate::runtime) fn atomic<T>(
        &self,
        operation: impl FnOnce(&Self) -> Result<T>,
    ) -> Result<T> {
        self.begin()?;
        let result = operation(self).and_then(|value| self.commit().map(|()| value));
        if let Err(error) = &result {
            self.rollback_after(error);
        }
        result
    }
    pub fn begin(&self) -> Result<()> {
        let depth = self.transaction_depth.get();
        #[cfg(test)]
        if depth == 0 {
            self.test_hooks.sql("begin", &self.connection).runtime()?;
        }
        self.execute_cached(&queries::begin_savepoint(depth), [])?;
        self.notices.borrow_mut().begin();
        self.transaction_depth.set(depth + 1);
        Ok(())
    }
    pub fn commit(&self) -> Result<()> {
        let depth = self
            .transaction_depth
            .get()
            .checked_sub(1)
            .ok_or(Error::database_error("No transaction."))?;
        if depth == 0 {
            #[cfg(test)]
            self.test_hooks.sql("commit", &self.connection).runtime()?;
            self.settle_joins()?;
        }
        self.execute_cached(&queries::release_savepoint(depth), [])?;
        self.transaction_depth.set(depth);
        self.notices.borrow_mut().commit();
        Ok(())
    }
    pub fn rollback(&self) -> Result<()> {
        self.healthy()?;
        let depth = self
            .transaction_depth
            .get()
            .checked_sub(1)
            .ok_or_else(|| Error::database_error("No transaction."))?;
        match self
            .sql()
            .execute_batch(&queries::rollback_savepoint(depth))
        {
            Ok(()) => {
                self.transaction_depth.set(depth);
                self.notices.borrow_mut().rollback();
                Ok(())
            }
            Err(error) => {
                self.notices.borrow_mut().discard();
                self.poison(&error);
                Err(error)
            }
        }
    }
    pub(in crate::runtime) fn rollback_after(&self, primary: &Error) {
        if let Err(secondary) = self.rollback() {
            cleanup_failure("rollback", primary, &secondary);
        }
    }
    /// PASSIVE, then TRUNCATE when no reader blocked it. Returns whether the WAL
    /// was fully copied, so that the next write restarts it.
    pub fn checkpoint(&self) -> Result<bool> {
        let timer = crate::observability::timing::Timer::start();
        let result = self.checkpoint_inner(true);
        timer.finish("application_checkpoint", result.is_ok());
        result
    }
    /// The writer's deferred checkpoint: PASSIVE only, so the WAL file keeps its size.
    pub fn checkpoint_passive(&self) -> Result<bool> {
        let timer = crate::observability::timing::Timer::start();
        let result = self.checkpoint_inner(false);
        timer.finish("deferred_checkpoint", result.is_ok());
        result
    }
    fn checkpoint_inner(&self, shrink: bool) -> Result<bool> {
        #[cfg(test)]
        self.test_hooks.checkpoint();
        #[cfg(test)]
        self.test_hooks
            .sql("checkpoint_passive", &self.connection)
            .runtime()?;
        let (busy, log, copied): (i64, i64, i64) =
            self.sql().query_row(queries::CHECKPOINT_PASSIVE, [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
        if busy != 0 {
            tracing::debug!(name: "store.checkpoint_deferred",
                 phase = "passive",
                 decision = "defer");
            return Ok(false);
        }
        if !shrink {
            // A reader's snapshot can pin frames; the rest is copied next time.
            return Ok(log == copied);
        }
        // busy_timeout remains zero: a reader can never block the writer here.
        #[cfg(test)]
        self.test_hooks
            .sql("checkpoint_truncate", &self.connection)
            .runtime()?;
        let busy: i64 = self
            .sql()
            .query_row(queries::CHECKPOINT_TRUNCATE, [], |r| r.get(0))?;
        if busy != 0 {
            tracing::debug!(name: "store.checkpoint_deferred",
                 phase = "truncate",
                 decision = "defer");
            return Ok(log == copied);
        }
        Ok(true)
    }
}

pub(super) fn cleanup_failure(phase: &'static str, primary: &Error, secondary: &Error) {
    tracing::error!(name: "store.cleanup_failed",
         phase = phase,
         code = primary.code(),
         primary = primary as &(dyn std::error::Error + 'static),
         secondary = secondary as &(dyn std::error::Error + 'static),
         sqlite_code = sqlite_code(secondary),
         decision = "discard_connection");
}

/// Only native numeric codes cross the privacy boundary; SQLite text can contain SQL.
pub(in crate::runtime) fn sqlite_code(error: &(dyn std::error::Error + 'static)) -> i32 {
    let mut cause = Some(error);
    for _ in 0..8 {
        let Some(current) = cause else { break };
        if let Some(rusqlite::Error::SqliteFailure(code, _)) =
            current.downcast_ref::<rusqlite::Error>()
        {
            return code.extended_code;
        }
        cause = current.source();
    }
    0
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
