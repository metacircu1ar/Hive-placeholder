//! Publish the complete empty schema before SQLite can open the final pathname.
use super::{Connection, Error, Result, check_files, database_admission, schema, state};
use crate::runtime::store::{STATEMENT_CACHE_CAPACITY, sql::RuntimeResult as _};
use atomic_fs::{Barriers, Step};
use std::{fs::File, io, path::Path};

struct Publication;
impl Barriers for Publication {
    #[cfg(test)]
    fn observe(&self, step: Step) -> io::Result<()> {
        tests::step(step)
    }
}

pub(super) fn open(path: &Path) -> Result<Connection> {
    // The helper's final raw descriptor close must precede every same-process
    // SQLite opener. Hold admission through connection configuration as well.
    let _admission = database_admission();
    if check_files(path)? {
        sync_parent(path, &Publication)?;
    } else {
        publish(path, &Publication)?;
    }
    let flags = rusqlite::OpenFlags::default() & !rusqlite::OpenFlags::SQLITE_OPEN_CREATE;
    let connection = Connection::open_with_flags(path, flags).runtime()?;
    connection
        .set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )
        .runtime()?;
    connection
        .busy_timeout(std::time::Duration::ZERO)
        .runtime()?;
    // Never initialize or configure an empty/unsupported existing file in place.
    schema::validate(&connection)?;
    schema::configure(&connection)?;
    connection.set_prepared_statement_cache_capacity(STATEMENT_CACHE_CAPACITY);
    connection
        .set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            false,
        )
        .runtime()?;
    Ok(connection)
}

fn publish(path: &Path, barriers: &impl Barriers) -> Result<()> {
    let memory = Connection::open_in_memory().runtime()?;
    schema::initialize(&memory)?;
    let bytes = memory.serialize(rusqlite::MAIN_DB).runtime()?;
    atomic_fs::PreparedCreate::new(
        path,
        (hive_setup_support::names::PUBLICATION_OPTIONS)
            .with_prefix(hive_setup_support::names::STATE_PREFIX),
    )
    .and_then(|prepared| prepared.publish_with(&*bytes, barriers))
    .map_err(|cause| Error::state_io_from(io::Error::other(cause)))
}

fn sync_parent(path: &Path, barriers: &impl Barriers) -> Result<()> {
    let parent = path
        .parent()
        .ok_or(Error::invalid_path("Missing database directory."))?;
    state::check_path(parent, true)?;
    // Also complete a previous attempt whose rename succeeded but sync failed.
    // Never raw-open the database itself: POSIX locks belong to the process.
    let sync = || -> io::Result<()> {
        barriers.observe(Step::BeforeDirectorySync)?;
        barriers.sync_directory(&File::open(parent)?)?;
        barriers.observe(Step::AfterDirectorySync)
    };
    sync().map_err(Error::state_io_from)
}

#[cfg(test)]
mod tests;
