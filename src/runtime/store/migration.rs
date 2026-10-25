//! Offline migration preflight never opens authoritative SQLite or its SHM.
mod queries;
use super::{schema, sql::RuntimeResult as _};
use crate::error::{Error, Result};
use hive_setup_support::migration::database::Copy;
use std::path::Path;

pub(crate) fn inspect(path: &Path) -> Result<()> {
    let _admission = super::database_admission();
    if !super::files::check_files(path)? {
        return Ok(());
    }
    let copied = Copy::open(path).map_err(Error::state_io_from)?;
    {
        let connection = rusqlite::Connection::open(copied.path()).runtime()?;
        schema::validate(&connection)?;
        let integrity: String = connection
            .query_row(queries::INSPECT_PRAGMA, [], |r| r.get(0))
            .runtime()?;
        if integrity != "ok" {
            return Err(Error::database_error(
                "Migration database integrity check failed.",
            ));
        }
    }
    copied.check_source().map_err(Error::state_io_from)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
