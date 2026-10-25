//! Existing writer and reader opening policies for the derived databases.
use super::{Index, SCHEMA, cache::Resident};
use crate::{Error, Result, disk, error::CommitExt};
use rusqlite::Connection;
use std::{path::Path, sync::Arc};

mod creation;
mod rebuilding;
#[cfg(test)]
mod tests;

static ADMISSION: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn admission() -> Result<std::sync::MutexGuard<'static, ()>> {
    ADMISSION.lock().map_err(|cause| {
        Error::shared_state_poisoned().caused_by(crate::error::Cause::poisoned(cause))
    })
}

const ATTACH_VECTORS: &str = "ATTACH DATABASE ?1 AS vector_cache";
const QUERY_ONLY: &str = "PRAGMA query_only=ON";
const WRITER_PRAGMAS: &str =
    "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;";
const SCHEMA_OBJECTS: &str = "SELECT type,name,coalesce(sql,'') FROM sqlite_schema WHERE name \
    NOT LIKE 'sqlite_%' ORDER BY type,name";

impl Index {
    pub(crate) fn inspect_migration(path: &Path) -> Result<()> {
        let _admission = admission()?;
        for (path, definition) in [
            (path.to_owned(), SCHEMA),
            (
                path.with_file_name("vectors.sqlite"),
                include_str!("vector_schema.sql"),
            ),
        ] {
            if check_admission_files(&path)? {
                let copied = hive_setup_support::migration::database::Copy::open(&path)?;
                let db = Connection::open(copied.path())?;
                check_schema(&db, &path, definition)?;
                let integrity: String = db.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
                if integrity != "ok" {
                    return Err(Error::migration_index_integrity());
                }
                drop(db);
                copied.check_source()?;
            }
        }
        Ok(())
    }

    pub(crate) fn open(path: &Path) -> Result<Self> {
        // Raw publication descriptors must close before any same-process SQLite
        // opener can acquire locks, including vector attachment and readers.
        let _admission = admission()?;
        let (connection, published) = open_writer(path, SCHEMA)?;
        let finish = || -> Result<Self> {
            let cache = path.with_file_name("vectors.sqlite");
            let (vectors, vectors_published) =
                open_writer(&cache, include_str!("vector_schema.sql"))?;
            drop(vectors);
            let attached = attach_vectors(&connection, path, &cache);
            if vectors_published {
                attached.committed()?;
            } else {
                attached?;
            }
            Ok(Self {
                connection,
                vectors: None,
            })
        };
        if published {
            finish().committed()
        } else {
            finish()
        }
    }

    pub(crate) fn reader(path: &Path, vectors: Option<Arc<Resident>>) -> Result<Self> {
        let _admission = admission()?;
        let cache = path.with_file_name("vectors.sqlite");
        check_admission_files(path)?;
        check_admission_files(&cache)?;
        let connection = readonly(path)?;
        attach_vectors(&connection, path, &cache)?;
        connection.execute_batch(QUERY_ONLY)?;
        Ok(Self {
            connection,
            vectors,
        })
    }

    pub(crate) fn with_vectors(self, vectors: Option<Arc<Resident>>) -> Self {
        Self { vectors, ..self }
    }
}

fn readonly(path: &Path) -> Result<Connection> {
    Ok(Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?)
}

fn open_writer(path: &Path, definition: &str) -> Result<(Connection, bool)> {
    let fresh = !check_admission_files(path)?;
    if fresh {
        creation::publish(path, definition)?;
    } else {
        disk::sync_publication_parent(path)?;
    }
    let open = || -> Result<(Connection, bool)> {
        let flags = (rusqlite::OpenFlags::default() & !rusqlite::OpenFlags::SQLITE_OPEN_CREATE)
            | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let connection = Connection::open_with_flags(path, flags)?;
        // A rejected WAL database must not be checkpointed merely on close.
        connection.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )?;
        check_schema(&connection, path, definition)?;
        connection.execute_batch(WRITER_PRAGMAS)?;
        connection.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            false,
        )?;
        Ok((connection, fresh))
    };
    if fresh { open().committed() } else { open() }
}

fn check_schema(connection: &Connection, path: &Path, definition: &str) -> Result<()> {
    let expected = Connection::open_in_memory()?;
    expected.execute_batch(definition)?;
    if schema(connection)? != schema(&expected)? {
        return Err(Error::unsupported_index(path));
    }
    Ok(())
}

fn existing_database(path: &Path, definition: &str) -> Result<Option<Connection>> {
    if !check_admission_files(path)? {
        return Ok(None);
    }
    let connection = readonly(path)?;
    check_schema(&connection, path, definition)?;
    Ok(Some(connection))
}

impl Index {
    /// Inspect selection under the caller's writer guard without recovery or an
    /// ATTACH that could create a missing vector cache.
    pub(crate) fn existing_space(
        path: &Path,
    ) -> Result<Option<(crate::embedding::Lease, crate::embedding::Space)>> {
        let _admission = admission()?;
        let connection = existing_database(path, SCHEMA)?;
        drop(existing_database(
            &path.with_file_name("vectors.sqlite"),
            include_str!("vector_schema.sql"),
        )?);
        match connection {
            Some(connection) => Self {
                connection,
                vectors: None,
            }
            .active_space(),
            None => Ok(None),
        }
    }
}

fn check_database_files(path: &Path) -> Result<bool> {
    let present = disk::exists(path)?;
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let candidate = companion(path, suffix);
        if disk::exists(&candidate)? {
            disk::check_file(&std::fs::symlink_metadata(candidate)?)?;
        }
    }
    Ok(present)
}

fn check_admission_files(path: &Path) -> Result<bool> {
    let present = check_database_files(path)?;
    if present {
        // SQLite can delete a WAL beside an empty main during its first read.
        if std::fs::symlink_metadata(path)?.len() == 0 {
            return Err(Error::unsupported_index(path));
        }
    } else {
        for suffix in ["-wal", "-shm", "-journal"] {
            if disk::exists(&companion(path, suffix))? {
                return Err(Error::unsupported_index(path));
            }
        }
    }
    Ok(present)
}

fn companion(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    name.into()
}

fn attach_vectors(connection: &Connection, main: &Path, path: &Path) -> Result<()> {
    if !check_admission_files(main)? || !check_admission_files(path)? {
        return Err(Error::unsupported_index(path));
    }
    connection.execute(
        ATTACH_VECTORS,
        [path.to_str().ok_or_else(Error::index_path_not_utf8)?],
    )?;
    Ok(())
}

fn schema(db: &Connection) -> Result<Vec<(String, String, String)>> {
    Ok(db
        .prepare(SCHEMA_OBJECTS)?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?)
}
