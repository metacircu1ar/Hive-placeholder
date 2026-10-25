use super::*;

fn fixture() -> (tempfile::TempDir, Store) {
    let temp = crate::runtime::test_directory();
    drop(
        State::new(temp.path().into())
            .unwrap()
            .initialize()
            .unwrap(),
    );
    state::directory(&temp.path().join("runtime")).unwrap();
    let store = Store::open(&temp.path().join("runtime/runtime.sqlite")).unwrap();
    (temp, store)
}

fn publish(path: &Path, bytes: &[u8]) {
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .create_new(path, bytes)
        .unwrap();
}

fn seed_history(store: &Store) {
    store.begin().unwrap();
    for (id, count, state, outcome, expired) in [
        (
            "many",
            1040,
            AgentState::Stopped,
            TaskOutcome::Completed,
            false,
        ),
        (
            "nonmonotonic",
            5,
            AgentState::Stopped,
            TaskOutcome::Completed,
            false,
        ),
        (
            "cancelled",
            3,
            AgentState::Stopped,
            TaskOutcome::Cancelled,
            false,
        ),
        ("expired", 3, AgentState::Stopped, TaskOutcome::Open, true),
        ("few", 1, AgentState::Stopped, TaskOutcome::Completed, false),
        ("live", 3, AgentState::Idle, TaskOutcome::Open, false),
        ("failed", 3, AgentState::Failed, TaskOutcome::Open, false),
        (
            "releasing",
            3,
            AgentState::Starting,
            TaskOutcome::Completed,
            false,
        ),
    ] {
        let mut a = agent(id, None);
        a.state = state;
        a.task_outcome = outcome;
        a.expired = expired;
        store.save(&a).unwrap();
        for n in 0..count {
            store.event(&a, EventKind::Spawned, json!({"n":n})).unwrap();
        }
    }
    store
        .connection
        .execute_batch(
            "UPDATE events SET at=0;
        DELETE FROM events WHERE seq=10;
        UPDATE events SET at=50 WHERE agent='nonmonotonic' AND seq=(
            SELECT seq FROM events WHERE agent='nonmonotonic' ORDER BY seq LIMIT 1 OFFSET 2)",
        )
        .unwrap();
    store.commit().unwrap();
}

fn seed_calls(store: &Store) {
    for (id, status, accepted, expired) in [
        ("pending", "accepted", 0, 0),
        ("inflight", "submitted", 0, 0),
        ("old", "completed", 0, 0),
        ("fresh", "completed", 50, 0),
        ("prior-expiry", "completed", 50, 1),
    ] {
        store.enqueue(id, "human", "many", "body").unwrap();
        store.track_call(id).unwrap();
        store
            .connection
            .execute(
                "UPDATE call_results SET accepted_at=?1 WHERE message_id=?2",
                params![accepted, id],
            )
            .unwrap();
        store
            .connection
            .execute(
                "UPDATE deliveries SET status=?1,call_expired=?2 WHERE message_id=?3",
                params![status, expired, id],
            )
            .unwrap();
    }
}

#[test]
fn preview_matches_multi_batch_execution_without_changing_history_or_outputs() {
    let (temp, store) = fixture();
    seed_history(&store);
    seed_calls(&store);
    let runtime = temp.path().join("runtime");
    let orphan = uuid::Uuid::new_v4().to_string();
    let pending = uuid::Uuid::new_v4().to_string();
    for id in [&orphan, &pending] {
        publish(
            &runtime.join(format!("output/{id}.json")),
            b"private output",
        );
    }
    store
        .connection
        .execute("INSERT INTO artifact_deletions VALUES(?1)", [&pending])
        .unwrap();
    publish(&runtime.join("output-manifest.jsonl"), b"old manifest\n");
    let preview = compact_preview(temp.path(), 50, 2).unwrap();
    assert!(preview.dry_run);
    assert_eq!(preview.events_to_remove, 1041);
    assert_eq!(preview.call_results_to_remove, 2);
    assert_eq!(preview.orphan_artifacts_to_remove, 1);
    assert_eq!(preview.pending_artifact_deletions, 1);
    assert_eq!(preview.attachment_recovery, "not_evaluated");
    assert_eq!(count(&store, "event_retention"), 0);
    assert_eq!(count(&store, "events"), 1060);
    assert_eq!(count(&store, "call_results"), 5);
    assert_eq!(count(&store, "artifact_deletions"), 1);
    for id in [&orphan, &pending] {
        assert_eq!(
            fs::read(runtime.join(format!("output/{id}.json"))).unwrap(),
            b"private output"
        );
    }
    assert_eq!(
        fs::read(runtime.join("output-manifest.jsonl")).unwrap(),
        b"old manifest\n"
    );
    drop(store);
    let result = compact(temp.path(), 50, 2).unwrap();
    assert_eq!(result.events_removed, preview.events_to_remove);
    assert_eq!(result.call_results_removed, preview.call_results_to_remove);
    assert_eq!(
        result.orphan_artifacts_removed,
        preview.orphan_artifacts_to_remove
    );
    let store = Store::open(&runtime.join("runtime.sqlite")).unwrap();
    assert_eq!(count(&store, "events"), 19);
    assert_eq!(count(&store, "call_results"), 3);
    assert_eq!(count(&store, "artifact_deletions"), 0);
    for id in [&orphan, &pending] {
        assert!(!runtime.join(format!("output/{id}.json")).exists());
    }
}

fn count(store: &Store, table: &str) -> i64 {
    store
        .connection
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn preview_does_not_attempt_attachment_recovery_or_initialize_missing_storage() {
    let (temp, store) = fixture();
    let untouched = temp.path().join("runtime/attachments/unknown-entry");
    publish(&untouched, b"recovery must inspect this later");
    drop(store);
    let preview = compact_preview(temp.path(), 0, 1).unwrap();
    assert_eq!(preview.events_to_remove, 0);
    assert_eq!(
        fs::read(&untouched).unwrap(),
        b"recovery must inspect this later"
    );
    let empty = crate::runtime::test_directory();
    drop(
        State::new(empty.path().into())
            .unwrap()
            .initialize()
            .unwrap(),
    );
    state::directory(&empty.path().join("runtime")).unwrap();
    assert!(compact_preview(empty.path(), 50, 1).is_err());
    assert!(!empty.path().join("runtime/runtime.sqlite").exists());
    assert!(!empty.path().join("runtime/output").exists());
}

#[test]
fn compact_bounds_fail_before_io_and_preview_keeps_lock_and_schema_checks() {
    let absent = crate::runtime::test_directory();
    let path = absent.path().join("absent");
    for (before, retained, field) in [
        (u64::MAX, 1, "--before"),
        (0, 0, "--keep-per-agent"),
        (0, u64::MAX, "--keep-per-agent"),
    ] {
        for error in [
            compact(&path, before, retained).unwrap_err(),
            compact_preview(&path, before, retained).unwrap_err(),
        ] {
            let error = serde_json::to_value(error).unwrap();
            assert_eq!(error["code"], "out_of_range");
            assert_eq!(error["field"], field);
        }
    }
    assert!(!path.exists());
    let (temp, store) = fixture();
    let lock = state::Lock::acquire(&temp.path().join("runtime/daemon.lock")).unwrap();
    assert_eq!(
        compact_preview(temp.path(), 0, 1).unwrap_err().code(),
        "profile_busy"
    );
    drop(lock);
    store
        .connection
        .execute_batch("CREATE TABLE unknown(x)")
        .unwrap();
    drop(store);
    assert_eq!(
        compact_preview(temp.path(), 0, 1).unwrap_err().code(),
        "unsupported_schema"
    );
}

#[test]
fn preview_rejects_an_unsafe_orphan_without_following_or_removing_it() {
    let (temp, store) = fixture();
    drop(store);
    let target = temp.path().join("private-target");
    publish(&target, b"keep");
    let orphan = temp
        .path()
        .join(format!("runtime/output/{}.json", uuid::Uuid::new_v4()));
    std::os::unix::fs::symlink(&target, &orphan).unwrap();
    assert_eq!(
        compact_preview(temp.path(), 0, 1).unwrap_err().code(),
        "unsafe_state"
    );
    assert!(orphan.symlink_metadata().unwrap().is_symlink());
    assert_eq!(fs::read(target).unwrap(), b"keep");
}
