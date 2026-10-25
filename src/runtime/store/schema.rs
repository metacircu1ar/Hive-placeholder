//! Current runtime schema and mailbox counters; no development-schema upgrades.
pub(super) mod queries;
use super::sql::{Database, RuntimeResult as _};
use crate::error::{Error, Result};
use rusqlite::Connection;

// These fields have dedicated tables, immutable stamps, or trigger-owned columns.
pub(super) const REMOVED_FIELDS: &[&str] =
    &["instructions", "ended_at", "branch_hold", "branch_phase"];

pub const HOT: &[&str] = &[
    "parent_id",
    "auth",
    "auth_group",
    "auth_checked_revision",
    "waiting_reason",
    "quota_wait_until",
    "lifetime",
    "state",
    "task_outcome",
    "paused",
    "expired",
    "thread_id",
    "execution_id",
    "turn_id",
    "control",
    "completion_pending",
    "result",
    "error",
    "provider_id",
    "provider_pid",
    "provider_spawn_before",
    "provider_spawn_after",
];

pub(crate) fn empty(db: &Connection) -> Result<bool> {
    Database::new(db).query_row(queries::EMPTY, [], |r| r.get(0))
}

pub(super) fn configure(db: &Connection) -> Result<()> {
    Database::new(db).execute_batch(queries::CONFIGURE_DATABASE)
}

pub fn initialize(db: &Connection) -> Result<()> {
    let empty = empty(db)?;
    if !empty {
        validate(db)?;
    }
    configure(db)?;
    if !empty {
        return Ok(());
    }
    db.execute_batch(queries::BEGIN_INITIALIZE).runtime()?;
    let initialized = (|| -> rusqlite::Result<()> {
        db.execute_batch(&queries::SCHEMA)?;
        db.pragma_update(None, "application_id", APPLICATION_ID)?;
        db.pragma_update(None, "user_version", crate::compatibility::STATE.current)?;
        db.execute_batch(queries::COMMIT_INITIALIZE)
    })();
    initialized.map_err(|error| {
        let primary = super::sql::db(error);
        if !db.is_autocommit()
            && let Err(error) = db.execute_batch(queries::ROLLBACK_INITIALIZE)
        {
            super::sql::cleanup_failure(
                "initialization_rollback",
                &primary,
                &super::sql::db(error),
            );
        }
        primary
    })
}

/// Read-only validation also protects offline credential pruning. Never repair
/// missing triggers or accept any schema other than the current definition.
pub(crate) fn validate(db: &Connection) -> Result<()> {
    identity(db)?;
    let reference = Connection::open_in_memory().runtime()?;
    reference.execute_batch(&queries::SCHEMA).runtime()?;
    if objects(db)? != objects(&reference)? {
        return Err(Error::unsupported_schema(concat!(
            "Unsupported or damaged Hive database format. Use a new state ",
            "directory; no automatic upgrade or repair is performed.",
        )));
    }
    Ok(())
}

pub(crate) fn identity(db: &Connection) -> Result<()> {
    let application: i64 = db
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .runtime()?;
    let format: i64 = db
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .runtime()?;
    if application == APPLICATION_ID && format > i64::from(crate::compatibility::STATE.current) {
        return Err(crate::compatibility::newer());
    }
    if application != APPLICATION_ID || format != i64::from(crate::compatibility::STATE.current) {
        return Err(Error::unsupported_schema(
            "Unsupported or inconsistent Hive database identity; no data was modified.",
        ));
    }
    Ok(())
}
const APPLICATION_ID: i64 = 0x4849_5645; // ASCII HIVE, not a format version.
super::rows::record! {
    #[derive(Debug, PartialEq, Eq)]
    #[cfg_attr(test, derive(serde::Serialize))]
    struct SchemaObject {
        name: String = 0,
        sql: String = 1,
    }
}
fn objects(db: &Connection) -> Result<Vec<SchemaObject>> {
    Database::new(db).query_rows(queries::OBJECTS, [])
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn compatibility_schema_matches_native_object_contract() {
        let db = Connection::open_in_memory().unwrap();
        initialize(&db).unwrap();
        crate::compatibility::contract("runtime-schema.json", &objects(&db).unwrap());
    }
    #[test]
    fn database_identity_requires_current_application_and_format() {
        let db = Connection::open_in_memory().unwrap();
        initialize(&db).unwrap();
        let application: i64 = db
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .unwrap();
        let format: i64 = db
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        crate::compatibility::contract(
            "database-marker.json",
            &serde_json::json!({"application_id": application, "user_version": format}),
        );
        for (application, format, code) in [
            (0x4c49_4e58, 1, "unsupported_schema"),
            (APPLICATION_ID, 2, "newer_format"),
            (APPLICATION_ID, 0, "unsupported_schema"),
            (0, 1, "unsupported_schema"),
        ] {
            db.pragma_update(None, "application_id", application)
                .unwrap();
            db.pragma_update(None, "user_version", format).unwrap();
            let before = db.serialize(rusqlite::MAIN_DB).unwrap().to_vec();
            assert_eq!(initialize(&db).unwrap_err().code(), code);
            assert_eq!(&*db.serialize(rusqlite::MAIN_DB).unwrap(), before);
        }
    }
    #[test]
    fn comb_binding_is_unindexed_and_obsolete_index_is_rejected() {
        let db = Connection::open_in_memory().unwrap();
        initialize(&db).unwrap();
        let generated: i64 = db
            .query_row(
                "SELECT hidden FROM pragma_table_xinfo('agents') WHERE name='comb_bound'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(generated, 2); // Retain the virtual generated column.
        let indexed: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_index_list('agents') i JOIN pragma_index_xinfo(i.name) c WHERE c.name='comb_bound')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!indexed);
        db.execute_batch("CREATE INDEX agents_core_ready ON agents(comb_bound, admission_class, pending_seq, id) WHERE runnable = 1;").unwrap();
        let before = objects(&db).unwrap();
        assert_eq!(initialize(&db).unwrap_err().code(), "unsupported_schema");
        assert_eq!(objects(&db).unwrap(), before);
    }

    #[test]
    fn current_schema_is_assembled_once_and_rejects_unknown_expressions() {
        for marker in [
            "{{future:new}}",
            "{{pending:future}}",
            "{{pending:new}",
            "pending:new}}",
        ] {
            assert!(std::panic::catch_unwind(|| queries::assemble(marker)).is_err());
        }
        let db = Connection::open_in_memory().unwrap();
        initialize(&db).unwrap();
        validate(&db).unwrap();
        let expected = objects(&db).unwrap();
        let reference = Connection::open_in_memory().unwrap();
        reference.execute_batch(&queries::SCHEMA).unwrap();
        assert_eq!(objects(&reference).unwrap(), expected);

        // Exact schema text is the current format, even for equivalent SQL.
        let altered = queries::SCHEMA.replacen("snapshot INTEGER", "snapshot  INTEGER", 1);
        assert_ne!(altered, *queries::SCHEMA);
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(&altered).unwrap();
        let before = objects(&db).unwrap();
        assert_eq!(validate(&db).unwrap_err().code(), "unsupported_schema");
        assert_eq!(objects(&db).unwrap(), before);
    }

    #[test]
    fn unsupported_and_damaged_schemas_are_rejected_without_repair() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE retained(data TEXT); INSERT INTO retained VALUES('evidence');",
        )
        .unwrap();
        let before = objects(&db).unwrap();
        assert_eq!(initialize(&db).unwrap_err().code(), "unsupported_schema");
        assert_eq!(objects(&db).unwrap(), before);
        assert_eq!(
            db.query_row("SELECT data FROM retained", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "evidence"
        );
        let old = Connection::open_in_memory().unwrap();
        old.execute_batch(&queries::SCHEMA).unwrap();
        old.execute_batch("CREATE TABLE blocked_profiles(auth TEXT PRIMARY KEY, error TEXT)")
            .unwrap();
        let before = objects(&old).unwrap();
        assert_eq!(initialize(&old).unwrap_err().code(), "unsupported_schema");
        assert_eq!(objects(&old).unwrap(), before);
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE sqliteXdata(value TEXT)")
            .unwrap();
        let before = objects(&db).unwrap();
        assert_eq!(initialize(&db).unwrap_err().code(), "unsupported_schema");
        assert_eq!(objects(&db).unwrap(), before);

        let db = Connection::open_in_memory().unwrap();
        initialize(&db).unwrap();
        initialize(&db).unwrap();
        db.execute_batch("DROP TRIGGER messages_ready_insert")
            .unwrap();
        let damaged = objects(&db).unwrap();
        assert_eq!(initialize(&db).unwrap_err().code(), "unsupported_schema");
        assert_eq!(objects(&db).unwrap(), damaged);
    }
}
