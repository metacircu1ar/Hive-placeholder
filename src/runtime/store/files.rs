//! Validate owned runtime files and open writer or read-only SQLite connections.
mod queries;
use super::{Store, schema, sql::RuntimeResult as _};
use crate::{
    error::{Error, Result},
    state,
};
use rusqlite::Connection;
mod creation;
use crate::runtime::locks::MutexExt as _;
static ADMISSION: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Exclude raw publication descriptor closes from same-process SQLite admission.
pub(crate) fn database_admission() -> std::sync::MutexGuard<'static, ()> {
    ADMISSION.lock_or_panic()
}
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    path::{Path, PathBuf},
    time::Duration,
};
// Control reads tolerate only a short SQLite busy wait, independently of writer work.
const READ_BUSY_TIMEOUT: Duration = Duration::from_millis(50);
pub struct Queries {
    pub(super) connection: Connection,
    pub(super) poison: RefCell<Option<Error>>,
    pub(super) artifacts_dir: PathBuf,
    pub(in crate::runtime) attachments_dir: PathBuf,
    #[cfg(test)]
    pub(super) test_hooks: crate::runtime::test_support::TestHooks,
}
/// Inspect only schema pages through SQLite; never copy history or open raw DB fds.
pub(crate) fn check_existing_format(path: &Path) -> Result<()> {
    if !check_files(path)? {
        return Ok(());
    }
    let connection = read_connection(path)?;
    schema::validate(&connection)
}
/// Profile operations need cross-file identity agreement, without assembling a
/// reference database on every metadata transaction. Runtime opens validate all SQL.
pub(crate) fn check_existing_identity(path: &Path) -> Result<()> {
    if !check_files(path)? {
        return Ok(());
    }
    schema::identity(&read_connection(path)?)
}
pub(crate) fn check_files(path: &Path) -> Result<bool> {
    let parent = path
        .parent()
        .ok_or(Error::invalid_path("Missing database directory."))?;
    match parent.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(Error::state_io_from(error)),
        Ok(metadata) => state::check_metadata(&metadata, true)?,
    }
    // Raw opens/closes would release another SQLite connection's process-owned
    // fcntl locks. Metadata checks do not disturb SQLite's descriptor bookkeeping.
    let (present, populated) = match path.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (false, false),
        Err(error) => return Err(Error::state_io_from(error)),
        Ok(metadata) => {
            state::check_metadata(&metadata, false)?;
            (true, metadata.len() != 0)
        }
    };
    for suffix in ["-wal", "-journal", "-shm"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        let sidecar = Path::new(&sidecar);
        let metadata = match sidecar.symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(Error::state_io_from(error)),
            Ok(metadata) => metadata,
        };
        if !populated {
            return Err(Error::recovery_required(concat!(
                "Runtime database is missing or empty but sidecar evidence remains; ",
                "use a fresh state directory and retain this history.",
            )));
        }
        state::check_metadata(&metadata, false)?;
        if suffix == "-journal" && metadata.len() != 0 {
            return Err(Error::recovery_required(concat!(
                "Runtime history has an unresolved rollback journal; retain it and ",
                "use a fresh state directory.",
            )));
        }
    }
    Ok(present)
}
impl Queries {
    #[cfg(test)]
    pub(in crate::runtime) fn test_connection(&self) -> &Connection {
        &self.connection
    }
    #[cfg(test)]
    pub(crate) fn test_hooks(&self) -> &crate::runtime::test_support::TestHooks {
        &self.test_hooks
    }
    pub(in crate::runtime) fn open_read_only(path: &Path) -> Result<Self> {
        if !check_files(path)? {
            return Err(Error::database_error("Runtime database is missing."));
        }
        let connection = read_connection(path)?;
        // Bound each SQLite busy wait on the reader only; writer timeout stays zero.
        connection.busy_timeout(READ_BUSY_TIMEOUT).runtime()?;
        connection
            .pragma_update(None, "query_only", true)
            .runtime()?;
        schema::validate(&connection)?;
        connection.set_prepared_statement_cache_capacity(super::STATEMENT_CACHE_CAPACITY);
        let artifacts_dir = output_directory(path)?;
        state::check_path(&artifacts_dir, true)?;
        state::check_path(&path.with_file_name("attachments"), true)?;
        Ok(Self {
            connection,
            poison: RefCell::default(),
            attachments_dir: path
                .with_file_name("attachments")
                .canonicalize()
                .map_err(Error::state_io_from)?,
            artifacts_dir,
            #[cfg(test)]
            test_hooks: crate::runtime::test_support::TestHooks::default(),
        })
    }
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = creation::open(path)?;
        let outputs = output_directory(path)?;
        state::directory(&outputs)?;
        let attachments_dir = path.with_file_name("attachments");
        state::directory(&attachments_dir)?;
        let attachments_dir = attachments_dir
            .canonicalize()
            .map_err(Error::state_io_from)?;
        let joins_possible = conn
            .query_row(queries::OPEN_SELECT, [], |r| r.get(0))
            .runtime()?;
        Ok(Self {
            #[cfg(feature = "comb")]
            collectors_possible: Cell::new(
                conn.query_row(queries::OPEN_SELECT_2, [], |r| r.get(0))
                    .runtime()?,
            ),
            joins_possible: Cell::new(joins_possible),
            queries: Queries {
                connection: conn,
                poison: RefCell::default(),
                artifacts_dir: outputs,
                attachments_dir,
                #[cfg(test)]
                test_hooks: crate::runtime::test_support::TestHooks::default(),
            },
            transaction_depth: Cell::new(0),
            notices: RefCell::default(),
            pending_wakes: RefCell::new(HashSet::new()),
            wake_all: Cell::new(false),
            deadlines_changed: Cell::new(false),
            observed: RefCell::new(HashSet::new()),
            observed_all: Cell::new(false),
            tree_changed: Cell::new(false),
        })
    }
}

#[cfg(test)]
thread_local! { static READ_CONNECTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn read_connection_count() -> usize {
    READ_CONNECTIONS.get()
}
/// The background checkpointer's connection. Read-write, because checkpointing
/// writes the database file, and `synchronous = FULL`, because the checkpointing
/// connection's setting governs that file's sync. It never closes last, so it
/// leaves the close-time checkpoint to the writer's connection.
pub(super) fn checkpoint_connection(path: &Path) -> Result<Connection> {
    let _admission = database_admission();
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
    connection
        .pragma_update(None, "synchronous", "FULL")
        .runtime()?;
    connection
        .pragma_update(None, "wal_autocheckpoint", 0)
        .runtime()?;
    Ok(connection)
}
fn read_connection(path: &Path) -> Result<Connection> {
    #[cfg(test)]
    READ_CONNECTIONS.set(READ_CONNECTIONS.get() + 1);
    let _admission = database_admission();
    let connection =
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).runtime()?;
    connection
        .set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )
        .runtime()?;
    Ok(connection)
}
fn output_directory(path: &Path) -> Result<PathBuf> {
    Ok(path
        .parent()
        .ok_or_else(|| Error::database_error("Missing database directory."))?
        .join("output"))
}
