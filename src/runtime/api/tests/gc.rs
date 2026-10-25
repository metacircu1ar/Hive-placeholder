use super::*;
use crate::config::Lifetime;

fn ended(c: &Core, id: &str, at: u64) -> Agent {
    let mut a = agent(id);
    a.state = AgentState::Stopped;
    a.task_outcome = TaskOutcome::Completed;
    c.store().save_at(&a, at).unwrap();
    c.store().agent(id).unwrap()
}
fn gc(c: &mut Core, id: &str, before: u64, after: Option<Value>, dry: bool) -> Value {
    let mut args = json!({"op":"gc","before":before,"dry_run":dry});
    if let Some(after) = after {
        args["after"] = after;
    }
    call(c, "human", id, args).unwrap()
}
fn db(t: &tempfile::TempDir) -> rusqlite::Connection {
    rusqlite::Connection::open(t.path().join("runtime.sqlite")).unwrap()
}

#[test]
fn gc_ending_timestamp_is_owned_once_and_survives_saves_rollback_and_recovery() {
    let t = crate::runtime::test_directory();
    let c = core(t.path());
    let mut a = agent("a");
    for state in [AgentState::Idle, AgentState::Stopped, AgentState::Failed] {
        a.state = state;
        a.paused = true;
        a.ended_at = Some(1); // An in-memory value has no authority.
        c.store().save_at(&a, 20).unwrap();
        assert_eq!(c.store().agent("a").unwrap().ended_at, None);
    }
    a.task_outcome = TaskOutcome::Completed;
    c.store().begin().unwrap();
    c.store().save_at(&a, 30).unwrap();
    assert_eq!(c.store().agent("a").unwrap().ended_at, Some(30));
    c.store().rollback().unwrap();
    assert_eq!(c.store().agent("a").unwrap().ended_at, None);
    c.store().save_at(&a, 40).unwrap();
    a.ended_at = None;
    a.expired = true;
    c.store().save_at(&a, 99).unwrap();
    c.store().recover_agents().unwrap();
    assert_eq!(c.store().agent("a").unwrap().ended_at, Some(40));
    let sql = db(&t);
    for update in [
        "ended_at=0",
        "ended_at=NULL",
        "ended_at=-1",
        "ended_at='bad'",
        "task_outcome='open',expired=0",
    ] {
        assert!(
            sql.execute(&format!("UPDATE agents SET {update} WHERE id='a'"), [])
                .is_err()
        );
    }
    let raw: String = sql
        .query_row("SELECT data FROM agents WHERE id='a'", [], |r| r.get(0))
        .unwrap();
    assert!(
        !serde_json::from_str::<Value>(&raw)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("ended_at")
    );
}

#[test]
fn gc_completion_termination_and_cascade_stamp_at_permanent_ending() {
    for complete in [false, true] {
        let t = crate::runtime::test_directory();
        let mut c = core(t.path());
        c.store().save(&agent("root")).unwrap();
        let mut child = agent("child");
        child.parent_id = Some("root".into());
        child.lifetime = Lifetime::WithParent;
        c.store().save(&child).unwrap();
        let mut old = ended(&c, "old", 3);
        old.parent_id = Some("root".into());
        old.lifetime = Lifetime::WithParent;
        c.store().save(&old).unwrap();
        if complete {
            call(
                &mut c,
                "human",
                "complete",
                json!({"op":"complete","target":"root","body":"done"}),
            )
            .unwrap();
            assert_eq!(c.store().agent("root").unwrap().ended_at, None);
            let mut parent = c.store().agent("root").unwrap();
            c.atomic(|c| c.finish_task(&mut parent)).unwrap();
        } else {
            call(
                &mut c,
                "human",
                "terminate",
                json!({"op":"terminate","target":"root"}),
            )
            .unwrap();
        }
        assert!(c.store().agent("root").unwrap().ended_at.is_some());
        assert!(c.store().agent("child").unwrap().ended_at.is_some());
        assert_eq!(c.store().agent("old").unwrap().ended_at, Some(3));
    }
}

#[test]
fn gc_uses_strict_end_age_keeps_resumable_children_and_skips_safety_blockers() {
    let t = crate::runtime::test_directory();
    let mut c = core(t.path());
    ended(&c, "old", 0);
    ended(&c, "equal", 10);
    ended(&c, "new", 11);
    let mut child = agent("child");
    child.parent_id = Some("old".into());
    c.store().save(&child).unwrap();
    for (id, state) in [
        ("stopped", AgentState::Stopped),
        ("failed", AgentState::Failed),
        ("idle", AgentState::Idle),
    ] {
        let mut a = agent(id);
        a.state = state;
        a.paused = true;
        c.store().save(&a).unwrap();
    }
    for id in ["claim", "fence", "input", "worker", "unclean"] {
        let mut a = ended(&c, id, 1);
        match id {
            "claim" => {
                db(&t)
                    .execute("UPDATE agents SET claimed=1 WHERE id=?1", [id])
                    .unwrap();
            }
            "fence" => {
                a.recovery_fence = Some(crate::runtime::domain::RecoveryFence::Failure(
                    "held".into(),
                ));
                c.store().save(&a).unwrap();
            }
            "input" => {
                c.store().enqueue("pending", "human", id, "work").unwrap();
            }
            "worker" => {
                c.started(id.into());
            }
            _ => {
                a.execution_id = Some("execution".into());
                c.store().save(&a).unwrap();
            }
        }
    }
    let preview = gc(&mut c, "preview", 10, None, true);
    assert_eq!(preview["stats"]["eligible_agents"], 1);
    assert_eq!(preview["stats"]["blocked_agents"], 5);
    assert_eq!(preview["stats"]["deleted_agents"], 0);
    assert_eq!(
        db(&t)
            .query_row("SELECT count(*) FROM requests", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let result = gc(&mut c, "delete", 10, None, false);
    assert_eq!(result["stats"]["deleted_agents"], 1);
    assert_eq!(c.store().agent("old").unwrap_err().code(), "agent_purged");
    for id in ["equal", "new", "child", "idle", "failed", "stopped"] {
        assert!(c.store().agent(id).is_ok());
    }
    assert_eq!(
        c.store().agent("child").unwrap().parent_id.as_deref(),
        Some("old")
    );
    assert_eq!(result["next_after"], Value::Null);
}

#[test]
fn gc_pages_tied_timestamps_advance_past_blockers_and_replay_lost_replies() {
    let t = crate::runtime::test_directory();
    let mut c = core(t.path());
    c.store().begin().unwrap();
    for i in 0..260 {
        let mut a = ended(&c, &format!("a-{i:03}"), 5);
        if i == 0 || i == 129 {
            a.recovery_fence = Some(crate::runtime::domain::RecoveryFence::Failure(
                "blocked".into(),
            ));
            c.store().save(&a).unwrap();
        }
    }
    c.store().commit().unwrap();
    let first = gc(&mut c, "page1", 6, None, false);
    assert_eq!(first["stats"]["examined_agents"], 128);
    assert_eq!(first["stats"]["deleted_agents"], 127);
    assert_eq!(first["selected_preview"].as_array().unwrap().len(), 20);
    // A concurrent purge removes one later candidate; retry still returns this page.
    call(
        &mut c,
        "human",
        "other",
        json!({"op":"purge","target":"a-128"}),
    )
    .unwrap();
    assert_eq!(gc(&mut c, "page1", 6, None, false), first);
    let second = gc(&mut c, "page2", 6, Some(first["next_after"].clone()), false);
    assert_eq!(second["stats"]["examined_agents"], 128);
    assert_eq!(second["stats"]["deleted_agents"], 127);
    let third = gc(
        &mut c,
        "page3",
        6,
        Some(second["next_after"].clone()),
        false,
    );
    assert_eq!(third["stats"]["deleted_agents"], 3);
    assert_eq!(third["next_after"], Value::Null);
    let retry = gc(&mut c, "new-pass", 6, None, false);
    assert_eq!(retry["stats"]["blocked_agents"], 2);
    assert_eq!(retry["stats"]["deleted_agents"], 0);
}

#[test]
fn gc_budget_splits_history_heavy_agents_and_allows_one_oversized_agent() {
    let t = crate::runtime::test_directory();
    let mut c = core(t.path());
    c.store().begin().unwrap();
    for id in ["a", "b", "c"] {
        ended(&c, id, 1);
    }
    c.store().commit().unwrap();
    let sql = db(&t);
    for (id, n) in [("a", 60_000), ("b", 60_000), ("c", 100_010)] {
        sql.execute("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<?2) INSERT INTO events(agent,at,kind,data) SELECT ?1,0,'turn_finished','{}' FROM n",rusqlite::params![id,n]).unwrap();
    }
    let mut after = None;
    for i in 0..3 {
        let page = gc(&mut c, &format!("p{i}"), 2, after, false);
        assert_eq!(page["stats"]["deleted_agents"], 1);
        assert!(page["stats"]["largest_agent_rows"].as_u64().unwrap() >= 60_000);
        after = page["next_after"]
            .as_object()
            .map(|_| page["next_after"].clone());
    }
    assert!(after.is_none());
}

#[test]
fn gc_transaction_failure_rolls_back_page_replay_and_outbox() {
    let t = crate::runtime::test_directory();
    let mut c = core(t.path());
    ended(&c, "a", 1);
    ended(&c, "b", 1);
    let sql = db(&t);
    sql.execute_batch("CREATE TRIGGER fail_gc BEFORE DELETE ON agents WHEN old.id='b' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    let args = json!({"op":"gc","before":2});
    assert!(call(&mut c, "human", "retry", args.clone()).is_err());
    assert!(c.store().agent("a").is_ok());
    assert!(c.store().agent("b").is_ok());
    for table in ["purged_agents", "requests", "artifact_deletions"] {
        assert_eq!(
            sql.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    sql.execute_batch("DROP TRIGGER fail_gc").unwrap();
    assert_eq!(
        call(&mut c, "human", "retry", args).unwrap()["stats"]["deleted_agents"],
        2
    );
}

#[test]
fn gc_arguments_and_agent_tools_are_strict() {
    let t = crate::runtime::test_directory();
    let mut c = core(t.path());
    c.store().save(&agent("caller")).unwrap();
    for (args, code, field) in [
        (json!({"op":"gc"}), "missing_argument", "/before"),
        (json!({"op":"gc","before":-1}), "invalid_value", "/before"),
        (
            json!({"op":"gc","before":u64::MAX}),
            "out_of_range",
            "/before",
        ),
        (
            json!({"op":"gc","before":2,"dry_run":null}),
            "invalid_value",
            "/dry_run",
        ),
        (
            json!({"op":"gc","before":2,"after":null}),
            "invalid_value",
            "/after",
        ),
        (
            json!({"op":"gc","before":2,"after":[1,"private-canary"]}),
            "invalid_value",
            "/after",
        ),
        (
            json!({"op":"gc","before":2,"after":{"ended_at":2,"id":"a"}}),
            "out_of_range",
            "/after/ended_at",
        ),
        (
            json!({"op":"gc","before":2,"after":{"ended_at":1,"id":""}}),
            "out_of_range",
            "/after/id",
        ),
        (
            json!({"op":"gc","before":2,"after":{"ended_at":1,"id":"é".repeat(32769)}}),
            "out_of_range",
            "/after/id",
        ),
    ] {
        control_inputs::assert_input(call(&mut c, "human", "bad", args), code, field, "gc");
        assert!(c.store().agent("caller").is_ok());
    }
    assert_eq!(
        call(&mut c, "caller", "internal", json!({"op":"gc","before":2}))
            .unwrap_err()
            .code(),
        "unsupported_operation"
    );
    assert_eq!(
        call(
            &mut c,
            "caller",
            "batch",
            json!({"op":"batch","operations":[{"op":"gc","before":2,"request_id":"gc"}]})
        )
        .unwrap_err()
        .code(),
        "invalid_value"
    );
    assert!(validate_tool("hive_gc", &json!({})).is_err());
    assert_eq!(
        gc(&mut c, "zero", 0, None, true)["stats"]["examined_agents"],
        0
    );
}

#[test]
fn gc_timestamp_consistency_rejects_null_and_forged_first_stamps() {
    let t = crate::runtime::test_directory();
    let c = core(t.path());
    c.store().save(&agent("live")).unwrap();
    let sql = db(&t);
    for update in [
        "expired=NULL,ended_at=1",
        "expired=NULL",
        "task_outcome=NULL",
        "ended_at=1",
        "ended_at='bad'",
        "task_outcome='completed'",
    ] {
        assert!(
            sql.execute(&format!("UPDATE agents SET {update} WHERE id='live'"), [])
                .is_err(),
            "{update}"
        );
    }
    assert_eq!(c.store().agent("live").unwrap().ended_at, None);
}

#[test]
fn gc_invalid_page_does_not_delete_and_accepted_replay_precedes_new_rules() {
    let t = crate::runtime::test_directory();
    let mut c = core(t.path());
    ended(&c, "old", 0);
    for (index, raw) in [
        json!({"op":"gc","before":1,"after":[0,"a"]}),
        json!({"op":"gc","before":1,"after":{"ended_at":0,"id":""}}),
        json!({"op":"gc"}),
    ]
    .into_iter()
    .enumerate()
    {
        assert!(call(&mut c, "human", "rejected", raw.clone()).is_err());
        assert!(c.store().agent("old").is_ok());
        assert!(
            c.store()
                .prior("human", "rejected", &raw.to_string())
                .unwrap()
                .is_none()
        );
        let id = format!("accepted-{index}");
        let accepted = json!({"accepted":"before-rule-change"});
        c.store()
            .remember("human", &id, &raw.to_string(), &accepted)
            .unwrap();
        assert_eq!(call(&mut c, "human", &id, raw).unwrap(), accepted);
    }
    for id in ["é".repeat(32768), " private\nidentity ".into()] {
        let args: GcArgs = serde_json::from_value(json!({"before":i64::MAX,
            "after":{"ended_at":0,"id":id}}))
        .unwrap();
        assert!(args.options().is_ok());
    }
}
