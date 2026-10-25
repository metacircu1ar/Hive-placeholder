use super::super::domain::{AgentState, EventKind, MessageState, TaskOutcome};
use super::*;
mod admission;
mod attachments;
mod branching;
mod calls;
mod checkpointer;
mod compaction;
mod counters;
#[cfg(not(feature = "comb"))]
mod features;
mod grouped_agent;
mod helpers;
mod joins;
mod numeric;
mod records;
mod redirect;
mod scheduled;
mod stalls;
mod steering;
mod temporaries;
mod timing;
use crate::{
    config::Config,
    runtime::Core,
    state::{self, State},
};
use rusqlite::Connection;
use serde_json::json;
use std::{collections::HashSet, fs, path::Path};
fn agent(id: &str, parent: Option<&str>) -> Agent {
    crate::runtime::test_support::AgentBuilder::new(id)
        .configure(|agent| agent.parent_id = parent.map(str::to_owned))
        .build()
}
fn core(path: &Path) -> Core {
    crate::runtime::test_support::CoreBuilder::new(path).build()
}
#[test]
fn cached_statements_read_current_rows_and_recover_after_nested_write_failure() {
    let store = crate::runtime::test_support::TestStore::new();
    let path = store.path();
    let mut a = agent("a", None);
    let mut b = agent("b", None);
    b.model = "other".into();
    std::sync::Arc::make_mut(&mut b.agent_options.baseline)
        .values
        .insert("model".into(), json!("other"));
    store.save(&a).unwrap();
    store.save(&b).unwrap();
    for id in ["a", "b", "a"] {
        assert_eq!(store.agent(id).unwrap().id, id);
    }
    assert!(matches!(
        store.agent("missing"),
        Err(Error::UnknownAgent { .. })
    ));
    assert_eq!(store.agent("a").unwrap().model, "mock");
    store.begin().unwrap();
    store.begin().unwrap();
    a.paused = true; // Stored column.
    a.model = "inside-savepoint".into(); // JSON identity, consistent with the private snapshot.
    std::sync::Arc::make_mut(&mut a.agent_options.baseline)
        .values
        .insert("model".into(), json!("inside-savepoint"));
    store.save(&a).unwrap();
    assert!(store.agent("a").unwrap().paused);
    assert_eq!(store.agent("a").unwrap().model, "inside-savepoint");
    store.enqueue("same-id", "human", "a", "first").unwrap();
    assert!(store.enqueue("same-id", "human", "b", "duplicate").is_err());
    store.rollback().unwrap();
    assert_eq!(store.transaction_depth.get(), 1);
    assert!(!store.agent("a").unwrap().paused);
    assert_eq!(store.agent("a").unwrap().model, "mock");
    assert!(store.next_message("a").unwrap().is_none());

    // Reuse the nested savepoint and failed write with different bindings, then release.
    store.begin().unwrap();
    b.paused = true;
    b.model = "committed".into();
    std::sync::Arc::make_mut(&mut b.agent_options.baseline)
        .values
        .insert("model".into(), json!("committed"));
    store.save(&b).unwrap();
    store
        .enqueue("same-id", "human", "b", "after-rollback")
        .unwrap();
    store.commit().unwrap();
    store.commit().unwrap();
    assert_eq!(store.transaction_depth.get(), 0);
    assert!(store.agent("b").unwrap().paused);
    assert_eq!(store.agent("b").unwrap().model, "committed");
    assert_eq!(store.agent("a").unwrap().model, "mock");
    let observer = Store::open(&path).unwrap();
    assert!(observer.agent("b").unwrap().paused);
    assert_eq!(observer.agent("b").unwrap().model, "committed");
    assert_eq!(
        observer.next_message("b").unwrap().unwrap().body,
        "after-rollback"
    );
}
#[test]
fn mailbox_triggers_cover_requeue_bulk_rollback_and_recovery() {
    let temp = crate::runtime::test_directory();
    let s = Store::open(&temp.path().join("db")).unwrap();
    s.save(&agent("root", None)).unwrap();
    s.save(&agent("a", Some("root"))).unwrap();
    s.enqueue_kind("seed", "human", "a", "start", true).unwrap();
    s.enqueue("mail", "root", "a", "reply").unwrap();
    let counts = || {
        s.connection
            .query_row(
                "SELECT pending_count,mail_count,pending_seq,head_startup FROM agents WHERE id='a'",
                [],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                    ))
                },
            )
            .unwrap()
    };
    assert_eq!(counts(), (2, 1, Some(1), Some(1)));
    let m = s.next_message("a").unwrap().unwrap();
    s.message_state(&m, MessageState::Submitting).unwrap();
    assert_eq!(counts(), (1, 1, Some(2), Some(0)));
    s.defer_submission("a").unwrap();
    assert_eq!(counts(), (2, 1, Some(1), Some(1)));
    s.begin().unwrap();
    s.undeliverable("a").unwrap();
    assert_eq!(counts(), (0, 0, None, None));
    s.rollback().unwrap();
    assert_eq!(counts(), (2, 1, Some(1), Some(1)));
    s.message_state(&m, MessageState::Submitted).unwrap();
    s.recover_agents().unwrap();
    assert_eq!(counts(), (1, 1, Some(2), Some(0)));
    assert_eq!(
        s.connection
            .query_row(
                "SELECT status FROM deliveries WHERE message_id='seed'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "uncertain"
    );
    assert_eq!(s.placement("a").unwrap(), 1);
}
#[test]
fn compaction_keeps_contiguous_history_provenance_replay_and_exact_output() {
    use std::os::unix::fs::PermissionsExt;
    let temp = crate::runtime::test_directory();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    drop(
        State::new(temp.path().into())
            .unwrap()
            .initialize()
            .unwrap(),
    );
    let runtime = temp.path().join("runtime");
    state::directory(&runtime).unwrap();
    let s = Store::open(&runtime.join("runtime.sqlite")).unwrap();
    let mut a = agent("done", None);
    a.state = AgentState::Stopped;
    a.task_outcome = TaskOutcome::Completed;
    s.save(&a).unwrap();
    s.enqueue("unfinished", "human", "done", "preserved")
        .unwrap();
    s.remember("human", "retry", "{}", &json!({"ok":true}))
        .unwrap();
    s.event(&a, EventKind::Spawned, json!({"v":1})).unwrap();
    let payload = json!({"text":"😀".repeat(10000)});
    s.begin().unwrap();
    s.event(&a, EventKind::AssistantMessage, payload.clone())
        .unwrap();
    s.commit().unwrap();
    let events = s.events("done", 0, 100).unwrap();
    let artifact = &events["events"][1]["data"]["artifact"];
    let id = artifact["id"].as_str().unwrap().to_owned();
    let mut offset = 0;
    let mut text = String::new();
    loop {
        let part = s
            .events(artifact["read_target"].as_str().unwrap(), offset, 101)
            .unwrap();
        text.push_str(part["text"].as_str().unwrap());
        offset = part["next_offset"].as_u64().unwrap();
        if part["eof"] == true {
            break;
        }
    }
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), payload);
    s.begin().unwrap();
    s.event(&a, EventKind::ToolCompleted, payload).unwrap();
    s.rollback().unwrap();
    s.event(&a, EventKind::TurnStarted, json!({})).unwrap();
    s.event(&a, EventKind::TurnFinished, json!({})).unwrap();
    s.connection
        .execute_batch(
            "UPDATE events SET at=0;UPDATE events SET at=9999 WHERE \
            kind='turn_started';",
        )
        .unwrap();
    drop(s);
    let compacted = compact(temp.path(), 1, 1).unwrap();
    assert_eq!(compacted.events_removed, 2);
    assert_eq!(compacted.orphan_artifacts_removed, 1);
    let s = Store::open(&runtime.join("runtime.sqlite")).unwrap();
    let page = s.events("done", 0, 1).unwrap();
    assert_eq!(page["history_expired"], true);
    assert_eq!(page["pruned_through"], 2);
    assert_eq!(page["events"][0]["type"], "turn_started");
    let next = s
        .events("done", page["cursor"].as_u64().unwrap(), 1)
        .unwrap();
    assert_eq!(next["events"][0]["type"], "turn_finished");
    assert!(s.next_message("done").unwrap().is_some());
    assert!(s.prior("human", "retry", "{}").unwrap().is_some());
    assert_eq!(s.output(&id, 0, 65536).unwrap()["eof"], true);
    assert_eq!(
        s.connection
            .query_row("SELECT count(*) FROM deliveries", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(runtime.join("output-manifest.jsonl").exists());
}

#[test]
fn idle_checkpoint_never_waits_for_a_long_reader() {
    let temp = crate::runtime::test_directory();
    let path = temp.path().join("db");
    let store = Store::open(&path).unwrap();
    store.save(&agent("first", None)).unwrap();
    store.checkpoint().unwrap();
    let reader = Connection::open(&path).unwrap();
    reader.execute_batch("BEGIN").unwrap();
    assert_eq!(
        reader
            .query_row("SELECT count(*) FROM agents", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    store.save(&agent("second", None)).unwrap();
    let began = std::time::Instant::now();
    store.checkpoint().unwrap();
    assert!(began.elapsed() < std::time::Duration::from_secs(1));
    assert_eq!(
        reader
            .query_row("SELECT count(*) FROM agents", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    reader.execute_batch("ROLLBACK").unwrap();
    store.checkpoint().unwrap();
}

#[test]
fn confirmed_release_and_provider_fence_commute_without_clearing_other_fences() {
    let tmp = crate::runtime::test_directory();
    let c = core(tmp.path());
    let mut a = agent("release", None);
    a.provider_process = Some(crate::runtime::domain::ProviderProcess::Reserved {
        id: "provider".into(),
    });
    a.execution_id = Some("execution".into());
    c.store().save(&a).unwrap();
    c.store().fence_provider("provider").unwrap();
    a = c.store().agent(&a.id).unwrap();
    assert_eq!(
        a.recovery_fence,
        Some(crate::runtime::domain::RecoveryFence::ExecutionCleanupUncertain)
    );
    c.store().record_conversation_release(&mut a, true).unwrap();
    c.store().fence_provider("provider").unwrap();
    assert!(c.store().agent(&a.id).unwrap().recovery_fence.is_none());
    assert!(
        !c.store()
            .connection
            .query_row("SELECT blocked FROM agents WHERE id=?1", [&a.id], |row| row
                .get::<_, bool>(0))
            .unwrap()
    );
    a.recovery_fence = Some(crate::runtime::domain::RecoveryFence::Failure(
        "credential_identity_changed".into(),
    ));
    c.store().save(&a).unwrap();
    c.store().record_conversation_release(&mut a, true).unwrap();
    assert_eq!(
        c.store().agent(&a.id).unwrap().recovery_fence,
        Some(crate::runtime::domain::RecoveryFence::Failure(
            "credential_identity_changed".into()
        ))
    );
    let mut stale = a.clone();
    a.execution_id = Some("new-execution".into());
    c.store().save(&a).unwrap();
    assert!(
        c.store()
            .record_conversation_release(&mut stale, true)
            .is_err()
    );
    assert_eq!(c.store().agent(&a.id).unwrap().execution_id, a.execution_id);
}

#[test]
fn credential_pruning_refuses_unsupported_schema_or_unproven_agent_pins() {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Defect {
        Schema,
        Missing,
        Null,
        Malformed,
        Unknown,
        Pinned,
    }

    // This spawn-free test makes immediate exclusive credential transitions;
    // unrelated test forks must not temporarily retain the preceding lock OFD.
    let _fork = crate::os::fork_guard().unwrap();
    for defect in [
        Defect::Schema,
        Defect::Missing,
        Defect::Null,
        Defect::Malformed,
        Defect::Unknown,
        Defect::Pinned,
    ] {
        let tmp = crate::runtime::test_directory();
        let state = State::new(tmp.path().join("state")).unwrap();
        let mut a = agent("saved", None);
        let profile = a.profile.clone();
        let binding = state.register("main", &profile).unwrap();
        let credential = binding.directory.join("provider/sentinel");
        fs::write(&credential, "retained").unwrap();
        let repair = state.begin_repair("main", &profile).unwrap();
        state.finish_repair(&repair, true, None).unwrap();
        state::directory(&state.root.join("runtime")).unwrap();
        let store = Store::open(&state.root.join("runtime/runtime.sqlite")).unwrap();
        a.auth_generation = binding.id;
        store.save(&a).unwrap();
        match defect {
            Defect::Schema => store
                .connection
                .execute_batch("DROP TRIGGER messages_ready_insert")
                .unwrap(),
            Defect::Missing => {
                store
                    .connection
                    .execute(
                        "UPDATE agents SET data=json_remove(data,'$.auth_generation')",
                        [],
                    )
                    .unwrap();
            }
            Defect::Null => {
                store
                    .connection
                    .execute(
                        "UPDATE agents SET data=json_set(data,'$.auth_generation',NULL)",
                        [],
                    )
                    .unwrap();
            }
            Defect::Malformed | Defect::Unknown => {
                let value = if defect == Defect::Unknown {
                    uuid::Uuid::new_v4().to_string()
                } else {
                    "invalid".into()
                };
                store
                    .connection
                    .execute(
                        "UPDATE agents SET data=json_set(data,'$.auth_generation',?1)",
                        [value],
                    )
                    .unwrap();
            }
            Defect::Pinned => {}
        }
        drop(store);
        assert!(state.prune("main", binding.id).is_err(), "{defect:?}");
        assert_eq!(fs::read_to_string(credential).unwrap(), "retained");
    }
}

#[test]
fn malformed_event_kind_and_payload_are_explicit_storage_errors() {
    for column in ["kind", "data"] {
        let tmp = crate::runtime::test_directory();
        let store = Store::open(&tmp.path().join("db")).unwrap();
        let a = agent("a", None);
        store.save(&a).unwrap();
        store.event(&a, EventKind::TurnFinished, json!({})).unwrap();
        // Inject on-disk corruption past the INSERT/UPDATE constraint.
        store
            .connection
            .pragma_update(None, "ignore_check_constraints", true)
            .unwrap();
        store
            .connection
            .pragma_update(None, "ignore_check_constraints", true)
            .unwrap();
        store
            .connection
            .execute(&format!("UPDATE events SET {column}='invalid'"), [])
            .unwrap();
        store
            .connection
            .pragma_update(None, "ignore_check_constraints", false)
            .unwrap();
        assert_eq!(
            store.events("a", 0, 10).unwrap_err().code(),
            "database_error"
        );
    }
}

#[test]
fn startup_refuses_old_wal_and_unsafe_sidecars_without_rewriting_history() {
    for defect in [
        "old_wal",
        "empty_wal",
        "missing_wal",
        "hot_journal",
        "symlink_shm",
    ] {
        let tmp = crate::runtime::test_directory();
        let path = tmp.path().join("runtime.sqlite");
        if matches!(defect, "old_wal" | "empty_wal") {
            state::private_file(&path, true).unwrap();
        }
        if defect == "old_wal" {
            let db = Connection::open(&path).unwrap();
            db.set_db_config(
                rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
                true,
            )
            .unwrap();
            db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
                .unwrap();
            schema::initialize(&db).unwrap();
            db.execute_batch("DROP TRIGGER messages_ready_insert")
                .unwrap();
            // Validate the actual WAL-visible schema before any writable setup.
        } else if matches!(defect, "hot_journal" | "symlink_shm") {
            drop(Store::open(&path).unwrap());
            if defect == "hot_journal" {
                let journal = tmp.path().join("runtime.sqlite-journal");
                state::private_file(&journal, true).unwrap();
                fs::write(journal, "unresolved").unwrap();
            } else {
                std::os::unix::fs::symlink(
                    tmp.path().join("missing"),
                    tmp.path().join("runtime.sqlite-shm"),
                )
                .unwrap();
            }
        } else {
            let wal = tmp.path().join("runtime.sqlite-wal");
            state::private_file(&wal, true).unwrap();
            fs::write(wal, "retained history").unwrap();
        }
        let snapshot = || {
            [
                "runtime.sqlite",
                "runtime.sqlite-wal",
                "runtime.sqlite-journal",
            ]
            .map(|name| crate::test_support::optional_bytes(tmp.path().join(name)))
        };
        let before = snapshot();
        assert!(Store::open(&path).is_err(), "{defect}");
        assert_eq!(snapshot(), before, "{defect}");
        if defect == "symlink_shm" {
            assert!(
                tmp.path()
                    .join("runtime.sqlite-shm")
                    .symlink_metadata()
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        }
    }
}

#[test]
#[ignore = "subprocess fixture"]
fn schema_inspection_lock_probe() {
    let path = std::env::var_os("HIVE_SCHEMA_LOCK_PROBE").expect("subprocess fixture input");
    let db = Connection::open(path).unwrap();
    db.busy_timeout(std::time::Duration::ZERO).unwrap();
    let error = db.execute_batch("BEGIN IMMEDIATE").unwrap_err();
    assert_eq!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy)
    );
}

#[test]
fn indexless_inspection_preserves_the_live_sqlite_writer_lock() {
    let tmp = crate::runtime::test_directory();
    let state = State::new(tmp.path().join("state")).unwrap();
    drop(state.initialize().unwrap());
    let runtime = state.root.join("runtime");
    state::directory(&runtime).unwrap();
    let path = runtime.join("runtime.sqlite");
    let store = Store::open(&path).unwrap();
    store.connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    state.check_format().unwrap();
    crate::test_support::run_isolated(
        crate::test_support::rerun_isolated(
            "runtime::store::tests::schema_inspection_lock_probe",
            true,
        )
        .env("HIVE_SCHEMA_LOCK_PROBE", &path)
        .stdin(std::process::Stdio::null()),
    );
    store.connection.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn continuation_metadata_constraint_rejects_null_kind_and_partial_metadata() {
    let dir = crate::runtime::test_directory();
    let store = Store::open(&dir.path().join("runtime.sqlite")).unwrap();
    store.save(&agent("agent", None)).unwrap();
    for (kind, root, attempt) in [
        (None, Some("root"), Some(1)),
        (Some("continuation"), None, Some(1)),
        (Some("continuation"), Some("root"), None),
        (Some("continuation"), Some("root"), Some(0)),
        (Some("approval_answer"), Some("root"), Some(1)),
    ] {
        assert!(store.connection.execute("INSERT INTO messages(id,sender,recipient,body,state,kind,continuation_root,continuation_attempt) VALUES('invalid','human','agent','bad','accepted',?1,?2,?3)",params![kind,root,attempt]).is_err());
    }
    store
        .enqueue_continuation("good", "agent", "continue", "root", 1)
        .unwrap();
}

#[test]
fn daemon_ownership_outlives_core_until_detached_service_releases_it() {
    let temp = crate::runtime::test_directory();
    let _fork = crate::os::fork_guard().unwrap();
    let core = core(temp.path());
    let path = core.state().root.join("runtime/daemon.lock");
    let owner = core.daemon_lock().clone();
    assert_eq!(
        crate::state::Lock::acquire(&path).err().unwrap().kind(),
        crate::error::ErrorCode::ProfileBusy,
    );
    drop(core);
    assert_eq!(
        crate::state::Lock::acquire(&path).err().unwrap().kind(),
        crate::error::ErrorCode::ProfileBusy,
    );
    drop(owner);
    assert!(crate::state::Lock::acquire(&path).is_ok());
}

#[test]
fn output_read_database_failure_has_runtime_context() {
    let temp = crate::runtime::test_directory();
    let store = Store::open(&temp.path().join("db")).unwrap();
    store
        .connection
        .execute_batch("DROP TABLE artifacts")
        .unwrap();
    let error = store
        .output(&uuid::Uuid::new_v4().to_string(), 0, 100)
        .unwrap_err();
    assert_eq!(error.code(), "database_error");
    assert_eq!(error.message(), "Cannot access the runtime database.");
    assert!(std::error::Error::source(&error).is_some());
}
