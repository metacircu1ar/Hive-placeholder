//! The background checkpointer never makes the writer meet `SQLITE_BUSY`, and the
//! writer's own checkpoint still completes and restarts the WAL.
use super::*;

fn fixture() -> (tempfile::TempDir, Store, Checkpointer) {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("runtime.sqlite")).unwrap();
    store.defer_checkpoints().unwrap();
    let checkpointer = store.start_checkpointer().unwrap();
    (temp, store, checkpointer)
}
fn commit_rows(store: &Store, round: usize) {
    store.begin().unwrap();
    for item in 0..8 {
        store
            .test_connection()
            .execute(
                "INSERT INTO instruction_snapshots(body) VALUES (?1)",
                [format!("{round}-{item}-{}", "x".repeat(4000))],
            )
            .unwrap();
    }
    store.commit().unwrap();
}
fn wal_bytes(temp: &tempfile::TempDir) -> u64 {
    match fs::metadata(temp.path().join("runtime.sqlite-wal")) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => panic!("cannot inspect the WAL: {error}"),
    }
}

#[test]
fn commits_never_meet_busy_while_the_checkpointer_loops() {
    let (temp, store, checkpointer) = fixture();
    for round in 0..400 {
        // Every commit asks for a pass, so passes overlap the next commits.
        commit_rows(&store, round);
        checkpointer.request();
    }
    checkpointer.stop();
    let count: i64 = store
        .test_connection()
        .query_row("SELECT count(*) FROM instruction_snapshots", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 3200);
    // The writer's connection closes last and its close-time checkpoint empties
    // the WAL.
    drop(store);
    assert_eq!(wal_bytes(&temp), 0);
}

#[test]
fn the_writers_own_checkpoint_completes_while_the_checkpointer_waits() {
    let (_temp, store, checkpointer) = fixture();
    for round in 0..200 {
        commit_rows(&store, round);
    }
    assert!(wal::frames() >= wal::THRESHOLD);
    {
        // Holding the pass lock parks the checkpointer before it enters SQLite.
        let _pass = checkpointer.exclusive();
        checkpointer.request();
        assert!(store.checkpoint_passive().unwrap());
        wal::checkpointed(true);
    }
    commit_rows(&store, 200);
    assert!(wal::frames() < wal::THRESHOLD, "the WAL restarted");
    checkpointer.stop();
}

#[test]
fn stopping_with_a_pending_request_joins_and_leaves_the_close_checkpoint() {
    let (temp, store, checkpointer) = fixture();
    for round in 0..50 {
        commit_rows(&store, round);
    }
    checkpointer.request();
    checkpointer.stop();
    drop(store);
    assert_eq!(wal_bytes(&temp), 0);
}

#[test]
fn a_drained_pass_restarts_the_wal_and_a_parked_checkpointer_times_out() {
    let (_temp, store, checkpointer) = fixture();
    for round in 0..200 {
        commit_rows(&store, round);
    }
    assert!(checkpointer.drain(std::time::Duration::from_secs(5)));
    wal::checkpointed(true);
    commit_rows(&store, 200);
    assert!(wal::frames() < wal::THRESHOLD, "the WAL restarted");
    for round in 201..400 {
        commit_rows(&store, round);
    }
    {
        // A stalled pass never holds the writer for longer than its wait.
        let _pass = checkpointer.exclusive();
        let began = std::time::Instant::now();
        assert!(!checkpointer.drain(std::time::Duration::from_millis(50)));
        assert!(began.elapsed() < std::time::Duration::from_secs(1));
    }
    checkpointer.stop();
}
