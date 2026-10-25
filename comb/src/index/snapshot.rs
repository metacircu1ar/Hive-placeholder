//! SQL side of publication-gated read snapshots.
use super::Index;
use crate::Result;
const BEGIN: &str = "BEGIN";
const PIN: &str = "SELECT (SELECT count(*) FROM embedding_space),(SELECT count(*) FROM \
    vector_cache.sqlite_schema)";
const ROLLBACK: &str = "ROLLBACK";
impl Index {
    pub(crate) fn pin_snapshot(&self) -> Result<()> {
        self.connection.execute_batch(BEGIN)?;
        // BEGIN alone does not acquire a WAL snapshot. Touch both databases
        // under the publication gate before allowing writers to proceed.
        if let Err(error) = self.connection.query_row(PIN, [], |_| Ok(())) {
            if let Err(secondary) = self.rollback_snapshot() {
                tracing::error!(name: "comb.snapshot_rollback_failed",
                     primary = &error as &(dyn std::error::Error + 'static),
                     secondary = &secondary as &(dyn std::error::Error + 'static),
                     decision = "discard_reader");
            }
            return Err(error.into());
        }
        Ok(())
    }
    pub(crate) fn rollback_snapshot(&self) -> Result<()> {
        Ok(self.connection.execute_batch(ROLLBACK)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_pin_rolls_back_before_returning_the_original_error() {
        let index = Index {
            connection: rusqlite::Connection::open_in_memory().unwrap(),
            vectors: None,
        };
        assert!(index.connection.is_autocommit());
        // Missing derived tables make the snapshot acquisition fail after BEGIN.
        let error = index.pin_snapshot().unwrap_err();
        assert_eq!(error.code, crate::ErrorCode::IndexError);
        assert!(index.connection.is_autocommit());
    }
}
