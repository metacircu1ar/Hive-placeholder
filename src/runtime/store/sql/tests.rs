use super::*;
use crate::runtime::{domain::EventKind, store::Store, test_support::agent};

#[test]
fn lifecycle_diagnostics_follow_commits_and_forget_rolled_back_work() {
    const CHILD: &str = "HIVE_TRANSACTION_LOG_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let (_, module) = module_path!().split_once("::").unwrap();
        let mut command = crate::test_support::rerun_isolated(
            &format!("{module}::lifecycle_diagnostics_follow_commits_and_forget_rolled_back_work"),
            false,
        );
        crate::test_support::run_isolated(command.env(CHILD, "1"));
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("diagnostics.jsonl");
    let (backend, guard) = hive_log::start(&path, hive_log::Config::default()).unwrap();
    backend.install_global().unwrap();
    let fixture = crate::runtime::test_support::TestStore::new();
    let value = agent();
    fixture.save(&value).unwrap();
    let event = |kind| {
        fixture
            .event(
                &value,
                kind,
                serde_json::json!({"text":"private-body-canary"}),
            )
            .unwrap();
    };
    fixture.begin().unwrap();
    event(EventKind::Spawned);
    fixture.begin().unwrap();
    event(EventKind::ExecutionFailed);
    fixture.rollback().unwrap();
    fixture.begin().unwrap();
    event(EventKind::BranchReady);
    fixture.commit().unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().is_empty());
    event(EventKind::TurnFinished); // Debug must be filtered before queuing metadata.
    fixture.commit().unwrap();
    fixture.test_hooks().at_sql("commit", |_| {
        Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR_WRITE),
            Some("private-sql-canary".into()),
        ))
    });
    assert!(
        fixture
            .atomic(|_| {
                event(EventKind::TaskCompleted);
                Ok(())
            })
            .is_err()
    );
    assert_eq!(fixture.transaction_depth.get(), 0);
    fixture.test_hooks().at_sql("commit", |_| Ok(()));
    fixture.begin().unwrap();
    event(EventKind::ApprovalRequested);
    fixture.commit().unwrap();
    assert_eq!(
        guard.finish(std::time::Duration::from_secs(2)).state,
        hive_log::Shutdown::Drained
    );
    let text = std::fs::read_to_string(path).unwrap();
    assert!(!text.contains("private-"));
    let rows: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let names: Vec<_> = rows.iter().map(|r| r["event"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        ["agent.spawned", "branch.ready", "approval.requested"]
    );
    assert!(rows.iter().all(|r| r["agent_id"] == value.id));
}

#[test]
fn snapshot_is_consistent_across_a_concurrent_commit_and_cannot_write() {
    let temp = crate::runtime::test_directory();
    let path = temp.path().join("runtime.sqlite");
    let store = Store::open(&path).unwrap();
    let mut value = agent();
    store.save(&value).unwrap();
    let reader = Queries::open_read_only(&path).unwrap();
    reader
        .snapshot(|queries| {
            assert!(!queries.agent("agent")?.paused);
            value.paused = true;
            store.save(&value)?;
            assert!(!queries.agent("agent")?.paused);
            Ok(())
        })
        .unwrap();
    assert!(reader.agent("agent").unwrap().paused);
    assert!(reader.connection.execute("DELETE FROM agents", []).is_err());
    assert!(reader.autocommit());
    let failure: Result<()> = reader.snapshot(|queries| {
        queries.agent("missing")?;
        Ok(())
    });
    assert!(failure.is_err());
    assert!(reader.autocommit());
    assert!(reader.agent("agent").unwrap().paused);
}

#[test]
fn rollback_failure_keeps_depth_and_poisons_all_sql_paths() {
    let fixture = crate::runtime::test_support::TestStore::new();
    fixture.begin().unwrap();
    fixture.begin().unwrap();
    fixture.sql().execute_batch("RELEASE tx_1").unwrap();
    let error = fixture.rollback().unwrap_err();
    assert_ne!(sqlite_code(&error), 0);
    assert_eq!(fixture.transaction_depth.get(), 2);
    assert!(fixture.healthy().is_err());
    assert!(fixture.begin().is_err());
    assert!(fixture.commit().is_err());
    assert!(
        fixture
            .sql()
            .query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
            .is_err()
    );
    assert!(
        fixture
            .sql()
            .cached()
            .execute("CREATE TABLE forbidden(x)", [])
            .is_err()
    );
    assert!(fixture.sql().execute_batch("ROLLBACK").is_err());
    assert!(fixture.use_memory_temporaries().is_err());
    assert_eq!(fixture.transaction_depth.get(), 2);
}

#[test]
fn atomic_error_survives_failed_rollback_and_reopen_is_healthy() {
    let fixture = crate::runtime::test_support::TestStore::new();
    let path = fixture.path();
    let failure: Result<()> = fixture.atomic(|s| {
        s.save(&agent())?;
        s.sql().execute_batch("RELEASE tx_0")?;
        Err(Error::invalid_request("primary operation"))
    });
    assert_eq!(failure.unwrap_err().code(), "invalid_request");
    assert!(fixture.healthy().is_err());
    // Reopen gives a new health state; an independently committed release is retained.
    let fresh = Store::open(&path).unwrap();
    assert!(fresh.healthy().is_ok());
    assert_eq!(fresh.agent("agent").unwrap().id, "agent");
}

#[test]
fn failed_snapshot_close_poisoned_even_if_sqlite_is_already_in_autocommit() {
    for primary in [false, true] {
        let fixture = crate::runtime::test_support::TestStore::new();
        let reader = Queries::open_read_only(&fixture.path()).unwrap();
        let result: Result<()> = reader.snapshot(|q| {
            q.connection.execute_batch("ROLLBACK").unwrap();
            if primary {
                Err(Error::invalid_request("original read error"))
            } else {
                Ok(())
            }
        });
        let error = result.unwrap_err();
        assert_eq!(
            error.code(),
            if primary {
                "invalid_request"
            } else {
                "database_error"
            }
        );
        assert!(reader.autocommit());
        assert!(reader.healthy().is_err());
        assert!(reader.snapshot(|_| Ok(())).is_err());
    }
}
