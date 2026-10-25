use super::*;
use crate::runtime::store::wal;

#[test]
fn writer_wal_policy_holds_and_debug_timings_are_silent_at_info() {
    const CHILD: &str = "HIVE_TIMING_TEST_LEVEL";
    let Ok(level) = std::env::var(CHILD) else {
        let (_, module) = module_path!().split_once("::").unwrap();
        for level in ["info", "debug"] {
            let mut child = crate::test_support::rerun_isolated(
                &format!("{module}::writer_wal_policy_holds_and_debug_timings_are_silent_at_info"),
                false,
            );
            crate::test_support::run_isolated(child.env(CHILD, level));
        }
        return;
    };
    let debug = level == "debug";
    let temp = crate::runtime::test_directory();
    let output = temp.path().join("timings.jsonl");
    let (backend, guard) = hive_log::start(
        &output,
        hive_log::Config {
            level: if debug {
                tracing::level_filters::LevelFilter::DEBUG
            } else {
                tracing::level_filters::LevelFilter::INFO
            },
            ..Default::default()
        },
    )
    .unwrap();
    backend.install_global().unwrap();
    compare_checkpoint_policy(temp.path());
    exercise_writer_and_barriers(temp.path());
    assert_eq!(
        guard.finish(std::time::Duration::from_secs(2)).state,
        hive_log::Shutdown::Drained
    );
    let text = fs::read_to_string(output).unwrap();
    assert!(!text.contains("private-canary"));
    let rows: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_timing_events(&rows, debug);
}

/// With the writer's hook, no COMMIT copies pages: the WAL keeps every frame until
/// the writer's deferred checkpoint, which then lets the next write restart it.
fn compare_checkpoint_policy(root: &Path) {
    for deferred in [false, true] {
        let c = Connection::open(root.join(if deferred { "deferred.db" } else { "native.db" }))
            .unwrap();
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA busy_timeout=0; CREATE TABLE t(id INTEGER PRIMARY KEY, value BLOB)").unwrap();
        if deferred {
            wal::install(&c).unwrap();
        }
        assert_eq!(threshold(&c), if deferred { 0 } else { 1000 });
        let mut batch = 0;
        let mut insert = |c: &Connection| {
            c.execute_batch("BEGIN").unwrap();
            for id in 0..32 {
                c.execute("INSERT INTO t VALUES(?1,zeroblob(4000))", [batch * 32 + id])
                    .unwrap();
            }
            c.execute_batch("COMMIT").unwrap();
            batch += 1;
        };
        for _ in 0..40 {
            insert(&c);
        }
        let (log, copied) = noop(&c);
        if deferred {
            assert!(wal::frames() >= wal::THRESHOLD);
            assert!(log >= wal::THRESHOLD && copied == 0, "{log} {copied}");
            assert_eq!(wal::frames(), log);
            let (busy, log, copied): (i32, i32, i32) = c
                .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .unwrap();
            assert_eq!((busy, copied), (0, log));
            wal::checkpointed(true);
            assert!(wal::frames() < wal::THRESHOLD);
            insert(&c);
            assert!(wal::frames() < wal::THRESHOLD, "the WAL restarted");
        } else {
            // SQLite's own policy copied pages inside some COMMIT.
            assert!(copied > 0 || log < wal::THRESHOLD, "{log} {copied}");
        }
        assert_eq!(
            c.query_row("SELECT count(*) FROM t", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            if deferred { 1312 } else { 1280 }
        );
    }
}
fn noop(c: &Connection) -> (i32, i32) {
    c.query_row("PRAGMA wal_checkpoint(NOOP)", [], |row| {
        Ok((row.get(1)?, row.get(2)?))
    })
    .unwrap()
}
fn threshold(connection: &Connection) -> i64 {
    connection
        .pragma_query_value(None, "wal_autocheckpoint", |row| row.get(0))
        .unwrap()
}
fn exercise_writer_and_barriers(root: &Path) {
    let fixture = crate::runtime::test_support::RuntimeFixture::new();
    fixture
        .hub
        .blocking_with(|core| core.store().save(&core.store().agent("agent")?))
        .unwrap();
    fixture.finish();
    let failed = crate::runtime::test_support::RuntimeFixture::new();
    failed
        .hub
        .blocking_with(|core| {
            core.store().test_hooks().at_sql("commit", |_| {
                Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR_WRITE),
                    Some("private-canary".into()),
                ))
            });
            Ok(())
        })
        .unwrap_err();
    assert_eq!(failed.finish_failed().code(), "database_error");
    let store = Store::open(&root.join("checkpoint.db")).unwrap();
    store.checkpoint().unwrap();
    crate::state::write_json(
        &root.join("private-canary.json"),
        &json!({"value":"private-canary"}),
    )
    .unwrap();
    let (stream, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
    let fd: std::os::fd::OwnedFd = stream.into();
    let file = std::fs::File::from(fd);
    assert!(crate::observability::state_sync::barrier(&file, "failed_barrier", false).is_err());
}
fn assert_timing_events(rows: &[Value], debug: bool) {
    let timing: Vec<_> = rows
        .iter()
        .filter(|r| {
            ["writer.batch", "runtime.timing", "state.sync"].contains(&r["event"].as_str().unwrap())
        })
        .collect();
    if !debug {
        assert!(timing.is_empty());
        return;
    }
    for event in ["writer.batch", "runtime.timing", "state.sync"] {
        assert!(
            timing.iter().any(|r| r["event"] == event),
            "missing {event}: {rows:?}"
        );
    }
    for row in &timing {
        let fields = row;
        let start = fields["start_ns"].as_u64().unwrap();
        let end = fields["end_ns"].as_u64().unwrap();
        assert!(end >= start);
        assert!(fields["duration_ns"].as_u64().is_some());
        if row["event"] == "writer.batch" {
            assert!(fields["jobs"].as_u64().unwrap() > 0);
            assert!(fields["commit_ns"].as_u64().is_some());
            assert!(
                ["none", "background", "restart", "deferred", "idle"]
                    .contains(&fields["checkpoint"].as_str().unwrap())
            );
            assert!(fields["frames"].as_u64().is_some());
        }
    }
    assert!(
        timing
            .iter()
            .any(|r| r["event"] == "state.sync" && r["success"] == false)
    );
    assert!(timing.iter().any(|r| r["event"] == "writer.batch"
        && r["phase"] == "rollback"
        && r["success"] == false));
    assert!(!rows.iter().any(|r| r["event"] == "log.dropped"));
}
