//! Explicit rebuild may discard a proven invalid derived vector cache.
use super::{
    Connection, Error, Index, Path, Result, admission, check_database_files, check_schema,
};
use std::error::Error as _;

impl Index {
    pub(crate) fn needs_automatic_rebuild(path: &Path) -> Result<bool> {
        let _admission = admission()?;
        if !super::check_admission_files(path)? {
            return Ok(false);
        }
        // This is normal writer startup under the root lock, not dry-run
        // inspection. SQLite must be able to roll back a valid hot journal
        // before deciding whether the derived schema needs rebuilding.
        let db = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
                | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        db.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )?;
        match check_schema(&db, path, super::SCHEMA) {
            Ok(()) => Ok(false),
            Err(error) if error.code == crate::ErrorCode::UnsupportedIndex => Ok(true),
            Err(error) => Err(error),
        }
    }
    pub(crate) fn require_preservable_vectors(path: &Path) -> Result<()> {
        let _admission = admission()?;
        let cache = path.with_file_name("vectors.sqlite");
        if !super::check_admission_files(&cache)? {
            return Ok(());
        }
        let copied = hive_setup_support::migration::database::Copy::open(&cache)?;
        inspect(&copied.path())?;
        copied.check_source()?;
        Ok(())
    }
    pub(crate) fn rebuild_space(
        path: &Path,
        automatic: bool,
    ) -> Result<Option<crate::embedding::Space>> {
        let _admission = admission()?;
        if !check_database_files(path)? || std::fs::symlink_metadata(path)?.len() == 0 {
            return Ok(None);
        }
        let copied = hive_setup_support::migration::database::Copy::open(path)?;
        let result = selection(&copied.path());
        copied.check_source()?;
        match result {
            Err(error)
                if !automatic
                    && (discardable(&error)
                        || matches!(
                            error.code,
                            crate::ErrorCode::InvalidJson
                                | crate::ErrorCode::InvalidEmbeddingConfig
                                | crate::ErrorCode::InvalidEmbeddingState
                        )) =>
            {
                // An explicit rebuild authorizes losing a malformed derived
                // selection. Automatic schema recovery must preserve it.
                Ok(None)
            }
            result => result,
        }
    }

    pub(crate) fn rebuild_vectors(path: &Path) -> Result<bool> {
        let _admission = admission()?;
        let cache = path.with_file_name("vectors.sqlite");
        check_database_files(path)?;
        if !check_database_files(&cache)? || std::fs::symlink_metadata(&cache)?.len() == 0 {
            return Ok(true);
        }
        match inspect(&cache) {
            Ok(()) => Ok(false),
            Err(error) if discardable(&error) => Ok(true),
            Err(error) => Err(error),
        }
    }
}

fn selection(path: &Path) -> Result<Option<crate::embedding::Space>> {
    use rusqlite::OptionalExtension;
    let db = Connection::open(path)?;
    let definition: Option<String> = db
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='table' AND name='embedding_space'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let Some(definition) = definition else {
        return Ok(None);
    };
    let reference = Connection::open_in_memory()?;
    reference.execute_batch(super::SCHEMA)?;
    let expected: String = reference.query_row(
        "SELECT sql FROM sqlite_schema WHERE type='table' AND name='embedding_space'",
        [],
        |r| r.get(0),
    )?;
    if definition != expected {
        return Err(Error::unsupported_index(path));
    }
    Ok(Index {
        connection: db,
        vectors: None,
    }
    .active_space()?
    .map(|(_, space)| space))
}

fn inspect(path: &Path) -> Result<()> {
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
        | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
        | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let db = Connection::open_with_flags(path, flags)?;
    db.set_db_config(
        rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
        true,
    )?;
    check_schema(&db, path, include_str!("../vector_schema.sql"))?;
    let mut check = db.prepare("PRAGMA quick_check")?;
    let rows = check.query_map([], |r| r.get::<_, String>(0))?;
    integrity(rows)
}

fn integrity(rows: impl IntoIterator<Item = rusqlite::Result<String>>) -> Result<()> {
    let mut valid = false;
    for row in rows {
        let row = row?;
        if row != "ok" {
            // SQLite may encode operational page-read failures as diagnostic
            // rows rather than returning an error. Text is not delete authority.
            return Err(Error::vector_cache_integrity().caused_by(IntegrityDiagnostic(row)));
        }
        valid = true;
    }
    if valid {
        Ok(())
    } else {
        Err(Error::vector_cache_integrity())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("SQLite integrity verification: {0}")]
struct IntegrityDiagnostic(String);

fn discardable(error: &Error) -> bool {
    if error.code == crate::ErrorCode::UnsupportedIndex {
        return true;
    }
    error
        .source()
        .and_then(|source| source.downcast_ref::<rusqlite::Error>())
        .and_then(rusqlite::Error::sqlite_error_code)
        .is_some_and(|code| {
            matches!(
                code,
                rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integrity_diagnostics_never_authorize_deletion_on_page_read_errors() {
        for text in [
            "*** in database main ***\nPage 2: unable to get the page. error code=10",
            "Main freelist: failed to get page 3",
            "Page 2: btreeInitPage() returns error code 11",
        ] {
            let error = integrity([Ok(text.to_owned())]).unwrap_err();
            assert!(!discardable(&error));
            assert_eq!(error.code, crate::ErrorCode::IndexError);
            assert_eq!(
                error
                    .source()
                    .unwrap()
                    .downcast_ref::<IntegrityDiagnostic>()
                    .unwrap()
                    .0,
                text
            );
            let public = serde_json::to_string(&error).unwrap();
            assert!(!public.contains(text));
            for remedy in [
                "cache was preserved",
                "Check storage health",
                "stop Comb",
                "<root>/.comb/vectors.sqlite",
                "-wal",
                "-shm",
                "-journal",
                "comb sync --rebuild",
            ] {
                assert!(error.message.contains(remedy));
            }
        }
        assert_eq!(
            integrity([]).unwrap_err().code,
            crate::ErrorCode::IndexError
        );
        assert!(integrity([Ok("ok".to_owned())]).is_ok());
        for (code, expected) in [
            (rusqlite::ffi::SQLITE_IOERR, false),
            (rusqlite::ffi::SQLITE_BUSY, false),
            (rusqlite::ffi::SQLITE_CORRUPT, true),
            (rusqlite::ffi::SQLITE_NOTADB, true),
        ] {
            let error = Error::from(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(code),
                None,
            ));
            assert_eq!(discardable(&error), expected);
        }
    }
}
