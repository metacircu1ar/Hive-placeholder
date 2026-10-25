use super::*;
use crate::runtime::domain::{EventKind, ReservedTurn};

fn ended(core: &Core, id: &str, parent: Option<&str>) -> Agent {
    let mut a = agent(id);
    a.parent_id = parent.map(str::to_owned);
    a.state = AgentState::Stopped;
    a.task_outcome = TaskOutcome::Completed;
    core.store().save(&a).unwrap();
    a
}
fn purge(core: &mut Core, id: &str, recursive: bool) -> Value {
    call(
        core,
        "human",
        &uuid::Uuid::new_v4().to_string(),
        json!({"op":"purge","target":id,"recursive":recursive}),
    )
    .unwrap()
}
fn db(temp: &tempfile::TempDir) -> rusqlite::Connection {
    rusqlite::Connection::open(temp.path().join("runtime.sqlite")).unwrap()
}
fn count(db: &rusqlite::Connection, sql: &str) -> i64 {
    db.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn purge_removes_owned_history_but_preserves_survivors_and_accepted_replays() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    seed_owned_purge_history(&c);
    let spawn = json!({"op":"spawn","role":"old","body":"original"});
    let batch =
        json!({"op":"batch","operations":[{"op":"spawn","role":"old","request_id":"child"}]});
    for (id, args) in [("old-spawn", &spawn), ("old-batch", &batch)] {
        c.store()
            .remember("human", id, &args.to_string(), &json!({"agent_id":"root"}))
            .unwrap();
    }
    let args = json!({"op":"purge","target":"root"});
    let result = call(&mut c, "human", "purge", args.clone()).unwrap();
    assert_eq!(result["counts"]["agents"], 1);
    assert_eq!(result["counts"]["messages"], 1);
    assert_eq!(result["counts"]["call_results"], 1);
    assert_eq!(result["counts"]["instruction_snapshots"], 0);
    assert_eq!(
        call(&mut c, "human", "purge", args.clone()).unwrap(),
        result
    );
    assert_eq!(
        call(&mut c, "human", "new-purge", args).unwrap_err().code(),
        "agent_purged"
    );
    for (id, args) in [("old-spawn", spawn), ("old-batch", batch)] {
        assert_eq!(
            call(&mut c, "human", id, args).unwrap(),
            json!({"agent_id":"root"})
        );
    }
    assert_eq!(
        call(
            &mut c,
            "human",
            "old-spawn",
            json!({"op":"spawn","role":"changed"})
        )
        .unwrap_err()
        .code(),
        "request_conflict"
    );
    assert_eq!(
        c.store().message_result("inbound").unwrap_err().code(),
        "unknown_message"
    );
    assert_eq!(c.store().agent("root").unwrap_err().code(), "agent_purged");
    assert_eq!(c.store().agent("typo").unwrap_err().code(), "unknown_agent");
    assert_eq!(
        c.store().agent("child").unwrap().parent_id.as_deref(),
        Some("root")
    );
    assert_eq!(
        c.store().next_message("child").unwrap().unwrap().body,
        "already accepted work"
    );
    let sql = db(&temp);
    assert_eq!(
        count(&sql, "SELECT count(*) FROM requests WHERE sender='root'"),
        0
    );
    assert_eq!(
        count(&sql, "SELECT count(*) FROM events WHERE agent='root'"),
        0
    );
    assert_eq!(
        count(
            &sql,
            "SELECT count(*) FROM deliveries WHERE message_id='outbound'"
        ),
        1
    );
    assert_eq!(count(&sql, "SELECT count(*) FROM instruction_snapshots"), 1);
    assert_eq!(c.store().active_count().unwrap(), 1);
}

#[test]
fn purge_recursive_traverses_tombstones_and_rejects_live_independent_descendants_atomically() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    ended(&c, "root", None);
    ended(&c, "middle", Some("root"));
    let mut leaf = agent("leaf");
    leaf.parent_id = Some("middle".into());
    c.store().save(&leaf).unwrap();
    purge(&mut c, "middle", false);
    let args = json!({"op":"purge","target":"root","recursive":true});
    assert_eq!(
        call(&mut c, "human", "reject", args.clone())
            .unwrap_err()
            .code(),
        "agent_busy"
    );
    assert!(c.store().agent("root").is_ok());
    let preview = call(
        &mut c,
        "human",
        "preview",
        json!({"op":"purge","target":"root","recursive":true,"dry_run":true}),
    )
    .unwrap();
    assert_eq!(preview["selected_agents"], 2);
    assert_eq!(preview["blocked_agents"], 1);
    assert_eq!(preview["blockers"][0]["agent_id"], "leaf");
    assert_eq!(
        count(
            &db(&temp),
            "SELECT count(*) FROM requests WHERE id IN ('preview','reject')"
        ),
        0
    );
    ended(&c, "leaf", Some("middle"));
    assert_eq!(
        call(&mut c, "human", "purge-tree", args).unwrap()["counts"]["agents"],
        2
    );
    ended(&c, "remaining", Some("middle")); // Fixture for later cleanup via a tombstoned root.
    assert_eq!(purge(&mut c, "root", true)["selected_agents"], 1);
    assert_eq!(purge(&mut c, "root", true)["selected_agents"], 0);
}

#[test]
fn purge_preconditions_preserve_unsafe_agents_and_allow_terminal_obsolete_reservations() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    for (i, reason) in [
        "agent_not_ended",
        "agent_cleanup_pending",
        "agent_cleanup_pending",
        "agent_cleanup_unconfirmed",
        "agent_cleanup_unconfirmed",
        "active_input",
        "unresolved_reservation",
    ]
    .iter()
    .enumerate()
    {
        let id = format!("case-{i}");
        let mut a = if i == 0 {
            agent(&id)
        } else {
            ended(&c, &id, None)
        };
        match i {
            0 => a.task_outcome = TaskOutcome::Open,
            1 => a.state = AgentState::Running,
            2 => {
                c.started(id.clone());
            }
            3 => {
                a.recovery_fence = Some(crate::runtime::domain::RecoveryFence::Failure(
                    "uncertain".into(),
                ));
            }
            4 => {
                a.provider_process = Some(crate::runtime::domain::ProviderProcess::Reserved {
                    id: "native".into(),
                });
                a.thread_id = Some("reserved-claude-id".into());
            }
            5 => c
                .store()
                .enqueue("queued", "human", &id, "pending")
                .unwrap(),
            6 => {
                a.reserved_turn = Some(ReservedTurn {
                    message_id: "missing".into(),
                    selection: a.desired_selection(),
                });
            }
            _ => unreachable!(),
        }
        c.store().save(&a).unwrap();
        let preview = call(
            &mut c,
            "human",
            "preview",
            json!({"op":"purge","target":id,"dry_run":true}),
        )
        .unwrap();
        assert_eq!(preview["blockers"][0]["reason"], *reason);
        assert_eq!(
            call(&mut c, "human", "reject", json!({"op":"purge","target":id}))
                .unwrap_err()
                .code(),
            "agent_busy"
        );
    }
    assert_terminal_obsolete_reservation_can_be_purged(&mut c);
}

fn assert_terminal_obsolete_reservation_can_be_purged(c: &mut Core) {
    let mut a = ended(c, "safe", None);
    a.provider_process = Some(crate::runtime::domain::ProviderProcess::Reserved {
        id: "old-provider".into(),
    });
    a.conversation_clean = true;
    a.reserved_turn = Some(ReservedTurn {
        message_id: "obsolete".into(),
        selection: a.desired_selection(),
    });
    c.store().save(&a).unwrap();
    c.store()
        .enqueue("obsolete", "human", "safe", "failed before send")
        .unwrap();
    c.store().undeliverable("safe").unwrap();
    assert_eq!(purge(c, "safe", false)["counts"]["agents"], 1);
}

#[test]
fn purge_rollback_retains_rows_replay_and_artifacts_and_can_be_retried() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    let a = ended(&c, "root", None);
    ended(&c, "child", Some("root"));
    c.store()
        .event(
            &a,
            EventKind::AssistantMessage,
            json!({"text":"x".repeat(20_000)}),
        )
        .unwrap();
    let sql = db(&temp);
    sql.execute_batch("CREATE TRIGGER fail_purge BEFORE DELETE ON events BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    let args = json!({"op":"purge","target":"root","recursive":true});
    assert_eq!(
        call(&mut c, "human", "retry", args.clone())
            .unwrap_err()
            .code(),
        "database_error"
    );
    for (table, expected) in [
        ("agents", 2),
        ("artifacts", 1),
        ("purged_agents", 0),
        ("artifact_deletions", 0),
        ("requests", 0),
    ] {
        assert_eq!(
            count(&sql, &format!("SELECT count(*) FROM {table}")),
            expected
        );
    }
    sql.execute_batch("DROP TRIGGER fail_purge").unwrap();
    assert_eq!(
        call(&mut c, "human", "retry", args).unwrap()["counts"]["agents"],
        2
    );
    let reopened = Store::open(&temp.path().join("runtime.sqlite")).unwrap();
    assert_eq!(reopened.agent("child").unwrap_err().code(), "agent_purged");
    assert_eq!(reopened.pending_artifact_deletions().unwrap().ids.len(), 1);
    reopened.finish_artifact_deletions().unwrap();
    assert!(
        reopened
            .pending_artifact_deletions()
            .unwrap()
            .ids
            .is_empty()
    );
}

#[test]
fn purge_artifacts_survive_event_pruning_and_cleanup_is_retryable_without_touching_peers() {
    use std::os::unix::fs::symlink;
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    let a = ended(&c, "root", None);
    let b = ended(&c, "peer", None);
    for owner in [&a, &a, &b] {
        c.store()
            .event(
                owner,
                EventKind::AssistantMessage,
                json!({"text":"x".repeat(20_000)}),
            )
            .unwrap();
    }
    let sql = db(&temp);
    let peer: String = sql
        .query_row("SELECT id FROM artifacts WHERE agent='peer'", [], |r| {
            r.get(0)
        })
        .unwrap();
    sql.execute("DELETE FROM events", []).unwrap(); // Compaction retains the artifact index.
    assert_eq!(purge(&mut c, "root", false)["counts"]["artifacts"], 2);
    let crate::runtime::store::ArtifactDeletions { dir, ids } =
        c.store().pending_artifact_deletions().unwrap();
    std::fs::remove_file(dir.join(format!("{}.json", ids[0]))).unwrap(); // Interrupted partial unlink.
    let unsafe_path = dir.join(format!("{}.json", ids[1]));
    std::fs::remove_file(&unsafe_path).unwrap();
    symlink(dir.join(format!("{peer}.json")), &unsafe_path).unwrap();
    assert!(c.store().finish_artifact_deletions().is_err());
    assert_eq!(c.store().pending_artifact_deletions().unwrap().ids.len(), 2);
    std::fs::remove_file(&unsafe_path).unwrap();
    c.store().finish_artifact_deletions().unwrap();
    assert!(c.store().output(&peer, 0, 100).is_ok());
    assert!(
        c.store()
            .pending_artifact_deletions()
            .unwrap()
            .ids
            .is_empty()
    );
}

#[test]
fn purge_listing_cursors_never_reuse_ids_or_sequences_and_options_are_strict() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    let original = ended(&c, "root", None);
    let sql = db(&temp);
    let cursor = count(&sql, "SELECT listing_seq FROM agents WHERE id='root'");
    for field in ["recursive", "dry_run"] {
        for value in [json!(null), json!("true"), json!(1)] {
            let mut args = json!({"op":"purge","target":"root"});
            args[field] = value;
            assert_eq!(
                call(&mut c, "human", "bad", args).unwrap_err().code(),
                "invalid_value"
            );
        }
    }
    c.store().save(&agent("caller")).unwrap();
    assert_eq!(
        call(
            &mut c,
            "caller",
            "no",
            json!({"op":"purge","target":"root"})
        )
        .unwrap_err()
        .code(),
        "unsupported_operation"
    );
    assert!(validate_tool("hive_purge", &json!({"target":"root"})).is_err());
    assert_eq!(call(&mut c,"human","batch-no",json!({"op":"batch","operations":[{"op":"purge","target":"root","request_id":"nested"}]})).unwrap_err().code(),"invalid_value");
    purge(&mut c, "root", false);
    assert!(c.store().save(&original).is_err());
    ended(&c, "new", None);
    let next = count(&sql, "SELECT listing_seq FROM agents WHERE id='new'");
    assert!(next > cursor);
    ended(&c, "caller", None);
    purge(&mut c, "caller", false);
    purge(&mut c, "new", false);
    ended(&c, "after-empty", None);
    assert!(
        count(
            &sql,
            "SELECT listing_seq FROM agents WHERE id='after-empty'"
        ) > next
    );
}

#[test]
fn purge_never_launched_proof_accounts_for_claude_reserved_session_ids() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    let mut a = ended(&c, "claude", None);
    a.profile.provider = crate::config::Provider::Claude;
    a.thread_id = Some(uuid::Uuid::new_v4().to_string());
    c.store().save(&a).unwrap();
    assert_eq!(purge(&mut c, "claude", false)["counts"]["agents"], 1);
    let mut started = ended(&c, "started", None);
    started.execution_id = Some("attempt".into());
    c.store().save(&started).unwrap();
    let preview = call(
        &mut c,
        "human",
        "preview",
        json!({"op":"purge","target":"started","dry_run":true}),
    )
    .unwrap();
    assert_eq!(
        preview["blockers"][0]["reason"],
        "agent_cleanup_unconfirmed"
    );
}

#[test]
fn purge_rejects_corrupt_artifact_outbox_and_unvalidated_child_identity() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    let a = ended(&c, "root", None);
    c.store()
        .event(
            &a,
            EventKind::AssistantMessage,
            json!({"text":"x".repeat(20_000)}),
        )
        .unwrap();
    let sql = db(&temp);
    let output: String = sql
        .query_row("SELECT id FROM artifacts", [], |r| r.get(0))
        .unwrap();
    sql.execute("INSERT INTO artifact_deletions VALUES(?1)", [&output])
        .unwrap();
    assert_eq!(
        c.store().finish_artifact_deletions().unwrap_err().code(),
        "database_error"
    );
    assert!(c.store().output(&output, 0, 100).is_ok());
    sql.execute("DELETE FROM artifact_deletions", []).unwrap();
    let mut corrupt = agent("");
    corrupt.parent_id = Some("root".into());
    c.store().save(&corrupt).unwrap();
    assert_eq!(
        call(
            &mut c,
            "human",
            "bad",
            json!({"op":"purge","target":"root","recursive":true})
        )
        .unwrap_err()
        .code(),
        "database_error"
    );
    assert!(c.store().agent("root").is_ok());
    assert!(c.store().agent("").is_ok());
}

#[test]
#[ignore = "bounded manual purge transaction measurement"]
fn purge_bounded_storage_measurement() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    c.atomic(|c| {
        for n in 0..100 {
            let a = ended(
                c,
                &format!("agent-{n:03}"),
                if n == 0 { None } else { Some("agent-000") },
            );
            for event in 0..10_000 {
                c.store().event(
                    &a,
                    EventKind::AssistantMessage,
                    json!({"text":"diagnostic output","event":event}),
                )?;
            }
        }
        Ok(())
    })
    .unwrap();
    let began = std::time::Instant::now();
    let result = purge(&mut c, "agent-000", true);
    eprintln!(
        "purge measurement: 100 agents, 1,000,000 events, transaction_seconds={:.6}",
        began.elapsed().as_secs_f64()
    );
    assert_eq!(result["counts"]["events"], 1_000_000);
    assert_eq!(result["counts"]["agents"], 100);
}

#[test]
fn purge_validates_every_page_before_deleting_any_selected_agent() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    c.atomic(|c| {
        ended(c, "root", None);
        for n in 0..259 {
            ended(c, &format!("child-{n:03}"), Some("root"));
        }
        let mut last = agent("child-259");
        last.parent_id = Some("root".into());
        c.store().save(&last)
    })
    .unwrap();
    let preview = call(
        &mut c,
        "human",
        "preview",
        json!({"op":"purge","target":"root","recursive":true,"dry_run":true}),
    )
    .unwrap();
    assert_eq!(preview["selected_agents"], 261);
    assert_eq!(preview["selected_preview"].as_array().unwrap().len(), 20);
    assert_eq!(preview["blocked_agents"], 1);
    assert_eq!(preview["blockers"][0]["agent_id"], "child-259");
    assert_eq!(
        call(
            &mut c,
            "human",
            "retry",
            json!({"op":"purge","target":"root","recursive":true})
        )
        .unwrap_err()
        .code(),
        "agent_busy"
    );
    assert_eq!(count(&db(&temp), "SELECT count(*) FROM agents"), 261);
    ended(&c, "child-259", Some("root"));
    assert_eq!(purge(&mut c, "root", true)["counts"]["agents"], 261);
}

#[test]
fn purge_preserves_child_completion_but_not_unexplained_missing_parent() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    ended(&c, "root", None);
    let mut child = agent("child");
    child.parent_id = Some("root".into());
    c.store().save(&child).unwrap();
    purge(&mut c, "root", false);
    // A surviving child can finish, while a truly missing parent stays a corruption error.
    child.completion_pending = Some("finished later".into());
    c.atomic(|c| c.finish_task(&mut child)).unwrap();
    let mut broken = agent("broken");
    broken.parent_id = Some("missing".into());
    broken.completion_pending = Some("done".into());
    c.store().save(&broken).unwrap();
    assert_eq!(
        c.atomic(|c| c.finish_task(&mut broken)).unwrap_err().code(),
        "unknown_agent"
    );
}

fn seed_owned_purge_history(c: &Core) {
    let root = ended(c, "root", None);
    let mut child = agent("child");
    child.parent_id = Some("root".into());
    c.store().save(&child).unwrap();
    c.store()
        .enqueue("outbound", "root", "child", "already accepted work")
        .unwrap();
    c.store()
        .enqueue("inbound", "human", "root", "old input")
        .unwrap();
    c.store().track_call("inbound").unwrap();
    c.store().undeliverable("root").unwrap();
    c.store()
        .event(&root, EventKind::TaskCompleted, json!({"result":"done"}))
        .unwrap();
    c.store()
        .remember("root", "owned-request", "{}", &json!({}))
        .unwrap();
}
