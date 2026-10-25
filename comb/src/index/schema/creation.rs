//! Serialize only a transactionally complete, fixed empty schema.
use super::{Connection, Error, Path, Result, disk};

pub(super) fn publish(path: &Path, definition: &str) -> Result<()> {
    let memory = Connection::open_in_memory()?;
    initialize(&memory, definition)?;
    let bytes = memory.serialize(rusqlite::MAIN_DB)?;
    disk::create_once(path, &bytes)
}

fn initialize(memory: &Connection, definition: &str) -> Result<()> {
    memory.execute_batch("BEGIN IMMEDIATE")?;
    let result = memory
        .execute_batch(definition)
        .and_then(|()| memory.execute_batch("COMMIT"));
    result.map_err(|primary| {
        if !memory.is_autocommit()
            && let Err(rollback) = memory.execute_batch("ROLLBACK")
        {
            // Keep both private native causes without replacing the public code.
            let primary = Error::from(primary);
            return primary
                .clone()
                .caused_by(InitializationFailure { primary, rollback });
        }
        Error::from(primary)
    })
}

#[derive(Debug, thiserror::Error)]
#[error("Schema initialization failed: {primary}; rollback failed: {rollback}")]
struct InitializationFailure {
    #[source]
    primary: Error,
    rollback: rusqlite::Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_schema_statement_failure_rolls_back_all_preceding_objects() {
        let memory = Connection::open_in_memory().unwrap();
        let error = initialize(&memory, "CREATE TABLE partial(value); INVALID SQL;").unwrap_err();
        assert_eq!(error.code, crate::ErrorCode::IndexError);
        assert!(memory.is_autocommit());
        assert!(super::super::schema(&memory).unwrap().is_empty());
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("index.sqlite");
        assert!(publish(&path, "CREATE TABLE partial(value); INVALID SQL;").is_err());
        assert!(
            std::fs::read_dir(directory.path())
                .unwrap()
                .next()
                .is_none()
        );
    }
}
