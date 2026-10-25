use super::*;
use crate::{approvals::ApprovalState, config::Lifetime, runtime::domain::MessageState};

fn running(c: &Core, id: &str) -> Agent {
    let mut a = agent(id);
    a.state = AgentState::Running;
    a.execution_id = Some(format!("exec-{id}"));
    a.turn_id = Some(format!("turn-{id}"));
    c.store().save(&a).unwrap();
    a
}
fn ask(c: &mut Core, agent: &str, request: &str) -> String {
    call(
        c,
        agent,
        request,
        json!({"op":"ask","message":"Proceed?","title":"Deploy","destructive":true}),
    )
    .unwrap()["approval_id"]
        .as_str()
        .unwrap()
        .into()
}
fn respond(c: &mut Core, id: &str, request: &str) -> Value {
    call(
        c,
        "human",
        request,
        json!({"op":"respond","approval_id":id,"action":"accept"}),
    )
    .unwrap()
}
fn db(t: &tempfile::TempDir) -> rusqlite::Connection {
    rusqlite::Connection::open(t.path().join("runtime.sqlite")).unwrap()
}
fn count(db: &rusqlite::Connection, table: &str) -> i64 {
    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn approvals_tool_schema_and_human_agent_boundaries_are_explicit() {
    let tool = tools()
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "hive_ask")
        .unwrap();
    assert_eq!(
        tool["inputSchema"]["properties"]["schema"],
        json!({"type":"object","additionalProperties":true,"description":"Optional flat elicitation form: at most 16 fields, 64 choices per field and 16384 JSON bytes."})
    );
    let args = json!({"request_id":"ask","message":"Choose","schema":{"type":"object","properties":{"x":{"type":"boolean"}}}});
    validate_tool("hive_ask", &args).unwrap();
    for field in [
        json!(null),
        json!("{}"),
        json!({"type":"object","properties":{},"$ref":"x"}),
    ] {
        let mut invalid = args.clone();
        invalid["schema"] = field;
        assert!(validate_tool("hive_ask", &invalid).is_err());
    }
    for name in ["hive_approvals", "hive_respond"] {
        assert!(validate_tool(name, &json!({})).is_err());
    }
    let t = crate::runtime::test_directory();
    let mut c = core(t.path());
    running(&c, "a");
    assert_eq!(
        call(&mut c, "human", "ask", json!({"op":"ask","message":"Hi"}))
            .unwrap_err()
            .code(),
        "unsupported_operation"
    );
    let id = ask(&mut c, "a", "ask");
    for raw in [
        json!({"op":"approvals"}),
        json!({"op":"approval","approval_id":id}),
        json!({"op":"respond","approval_id":id,"action":"cancel"}),
    ] {
        assert_eq!(
            call(&mut c, "a", "forbidden", raw).unwrap_err().code(),
            "unsupported_operation"
        );
    }
}

#[test]
fn approvals_supersession_replay_and_invalid_new_asks_preserve_pending_question() {
    let t = crate::runtime::test_directory();
    let mut c = core(t.path());
    running(&c, "a");
    let first = ask(&mut c, "a", "first");
    let second = ask(&mut c, "a", "second");
    assert_eq!(ask(&mut c, "a", "first"), first);
    assert_eq!(
        c.store().pending_approval("a").unwrap().as_deref(),
        Some(second.as_str())
    );
    assert_eq!(
        c.store().approval(&first).unwrap().state,
        ApprovalState::Superseded
    );
    assert!(call(&mut c,"a","bad",json!({"op":"ask","message":"Hi","schema":{"type":"object","properties":{"x":{"type":"object"}}}})).is_err());
    assert_eq!(
        c.store().pending_approval("a").unwrap(),
        Some(second.clone())
    );
    assert_eq!(
        call(
            &mut c,
            "human",
            "old",
            json!({"op":"respond","approval_id":first,"action":"accept"})
        )
        .unwrap_err()
        .code(),
        "approval_closed"
    );
    assert_eq!(count(&db(&t), "approvals"), 2);
    assert_eq!(
        call(
            &mut c,
            "a",
            "first",
            json!({"op":"ask","message":"different"})
        )
        .unwrap_err()
        .code(),
        "request_conflict"
    );
}

#[test]
fn missing_ask_content_defers_to_lifecycle_and_replay_without_mutation() {
    let t = crate::runtime::test_directory();
    let mut c = core(t.path());
    let mut a = running(&c, "a");
    let first = ask(&mut c, "a", "first");
    let error = call(&mut c, "a", "missing", json!({"op":"ask"})).unwrap_err();
    let wire = serde_json::to_value(error).unwrap();
    assert_eq!(wire["code"], "missing_argument");
    assert_eq!(wire["field"], "/message");
    assert_eq!(c.store().pending_approval("a").unwrap(), Some(first));
    assert_eq!(count(&db(&t), "approvals"), 1);
    assert_eq!(
        call(&mut c, "a", "first", json!({"op":"ask"}))
            .unwrap_err()
            .code(),
        "request_conflict"
    );
    a.state = AgentState::Idle;
    a.turn_id = None;
    c.store().save(&a).unwrap();
    assert_eq!(
        call(&mut c, "a", "inactive", json!({"op":"ask"}))
            .unwrap_err()
            .code(),
        "no_active_turn"
    );
    assert_eq!(
        call(&mut c, "human", "human", json!({"op":"ask"}))
            .unwrap_err()
            .code(),
        "unsupported_operation"
    );
}

#[test]
fn approvals_form_validation_and_actions_never_apply_defaults() {
    let t = crate::runtime::test_directory();
    let mut c = core(t.path());
    running(&c, "a");
    let schema = json!({"type":"object","properties":{"text":{"type":"string","minLength":2},"optional":{"type":"boolean","default":true}},"required":["text"]});
    let id = call(
        &mut c,
        "a",
        "form",
        json!({"op":"ask","message":"Choose","schema":schema}),
    )
    .unwrap()["approval_id"]
        .as_str()
        .unwrap()
        .to_owned();
    for content in [
        json!({}),
        json!({"text":"x"}),
        json!({"text":"ok","extra":1}),
        Value::Null,
    ] {
        assert_eq!(
            call(
                &mut c,
                "human",
                "bad",
                json!({"op":"respond","approval_id":id,"action":"accept","content":content})
            )
            .unwrap_err()
            .code(),
            "invalid_value"
        );
        assert_eq!(
            c.store().approval(&id).unwrap().state,
            ApprovalState::Pending
        );
    }
    let raw = json!({"op":"respond","approval_id":id,"action":"accept","content":{"text":"éé"}});
    let answer = call(&mut c, "human", "answer", raw.clone()).unwrap();
    assert_eq!(call(&mut c, "human", "answer", raw).unwrap(), answer);
    assert_eq!(
        c.store().approval(&id).unwrap().content,
        Some(json!({"text":"éé"}))
    );
    assert_eq!(count(&db(&t), "messages"), 1);
    for action in ["decline", "cancel"] {
        let id = ask(&mut c, "a", action);
        assert!(
            call(
                &mut c,
                "human",
                "content-forbidden",
                json!({"op":"respond","approval_id":id,"action":action,"content":{}})
            )
            .is_err()
        );
        let mut raw = json!({"op":"respond","approval_id":id,"action":action});
        if action == "decline" {
            raw["reason"] = json!("Try staging first");
        }
        call(&mut c, "human", action, raw).unwrap();
        assert_eq!(
            c.store().approval(&id).unwrap().state,
            ApprovalState::Answered
        );
    }
}

#[test]
fn approvals_answers_have_priority_without_interrupting_and_track_message_results() {
    let t = crate::runtime::test_directory();
    let mut c = core(t.path());
    let a = running(&c, "a");
    c.store()
        .enqueue_kind("startup", "human", "a", "start", true)
        .unwrap();
    c.store().enqueue("ordinary", "human", "a", "peer").unwrap();
    // Existing redirects retain their position ahead of the later approval answer.
    call(
        &mut c,
        "human",
        "redirect",
        json!({"op":"send","target":"a","body":"earlier","mode":"redirect"}),
    )
    .unwrap();
    let id = ask(&mut c, "a", "ask");
    let sql = db(&t);
    let interrupts = count(&sql, "redirect_interrupts");
    let result = respond(&mut c, &id, "answer");
    assert_eq!(count(&sql, "redirect_interrupts"), interrupts);
    assert_eq!(c.store().agent("a").unwrap().turn_id, a.turn_id);
    let mut bodies = Vec::new();
    while let Some(m) = c.store().next_message("a").unwrap() {
        bodies.push(m.body.clone());
        c.store()
            .message_state(&m, MessageState::Submitted)
            .unwrap();
    }
    assert_eq!(bodies[0], "start");
    assert_eq!(bodies[1], "earlier");
    assert!(bodies[2].starts_with("Approval "));
    assert_eq!(bodies[3], "peer");
    let page = c.store().messages("a", None, 200).unwrap();
    let message = page["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == result["message_id"])
        .unwrap();
    assert_eq!(message["kind"], "approval_answer");
    assert!(message.get("mode").is_none());
    assert_eq!(count(&sql, "call_results"), 2); // ordinary redirect + approval, plus no tracking for direct fixture startup
}

#[test]
fn approvals_response_rollback_restores_question_wakeup_mail_and_events() {
    for table in ["messages", "events", "requests"] {
        let t = crate::runtime::test_directory();
        let mut c = core(t.path());
        let mut a = running(&c, "a");
        let id = ask(&mut c, "a", "ask");
        a.state = AgentState::Stopped;
        c.store().save(&a).unwrap();
        let sql = db(&t);
        let events = count(&sql, "events");
        sql.execute_batch(&format!("CREATE TRIGGER fail_answer BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'injected'); END")).unwrap();
        let raw = json!({"op":"respond","approval_id":id,"action":"accept"});
        assert!(call(&mut c, "human", "answer", raw.clone()).is_err());
        assert_eq!(
            c.store().approval(&id).unwrap().state,
            ApprovalState::Pending
        );
        assert_eq!(c.store().agent("a").unwrap().state, AgentState::Stopped);
        for table in ["messages", "deliveries", "call_results"] {
            assert_eq!(count(&sql, table), 0);
        }
        assert_eq!(count(&sql, "events"), events);
        sql.execute_batch("DROP TRIGGER fail_answer").unwrap();
        call(&mut c, "human", "answer", raw).unwrap();
        assert_eq!(c.store().agent("a").unwrap().state, AgentState::Starting);
    }
}

#[test]
fn approvals_stop_pause_and_failure_preserve_question_and_only_unpaused_stopped_wakes() {
    for (state, paused) in [
        (AgentState::Stopped, false),
        (AgentState::Stopped, true),
        (AgentState::Failed, false),
    ] {
        let t = crate::runtime::test_directory();
        let mut c = core(t.path());
        let mut a = running(&c, "a");
        let id = ask(&mut c, "a", "ask");
        a.state = state;
        a.paused = paused;
        c.store().save(&a).unwrap();
        assert_eq!(c.store().pending_approval("a").unwrap(), Some(id.clone()));
        respond(&mut c, &id, "answer");
        let expected = if state == AgentState::Stopped && !paused {
            AgentState::Starting
        } else {
            state
        };
        assert_eq!(c.store().agent("a").unwrap().state, expected);
        assert_eq!(c.store().agent("a").unwrap().paused, paused);
    }
}

#[test]
fn approvals_terminal_cascade_withdraws_pending_but_preserves_answered_history_and_replay() {
    for complete in [false, true] {
        let t = crate::runtime::test_directory();
        let mut c = core(t.path());
        let mut root = running(&c, "root");
        let root_id = ask(&mut c, "root", "ask");
        let answer = respond(&mut c, &root_id, "answer");
        let mut ids = Vec::new();
        for (id, parent) in [("child", "root"), ("grandchild", "child")] {
            let mut a = running(&c, id);
            a.parent_id = Some(parent.into());
            a.lifetime = Lifetime::WithParent;
            c.store().save(&a).unwrap();
            ids.push(ask(&mut c, id, "ask"));
        }
        let pending = ask(&mut c, "root", "pending");
        ids.push(pending.clone());
        if complete {
            root.completion_pending = Some("done".into());
            c.atomic(|c| c.finish_task(&mut root)).unwrap();
        } else {
            call(
                &mut c,
                "human",
                "end",
                json!({"op":"terminate","target":"root"}),
            )
            .unwrap();
        }
        for id in ids {
            assert_eq!(
                c.store().approval(&id).unwrap().state,
                ApprovalState::Withdrawn
            );
        }
        assert_eq!(respond(&mut c, &root_id, "answer"), answer);
        assert_eq!(
            c.store().approval(&root_id).unwrap().state,
            ApprovalState::Answered
        );
        assert_eq!(
            call(
                &mut c,
                "human",
                "late",
                json!({"op":"respond","approval_id":pending,"action":"accept"})
            )
            .unwrap_err()
            .code(),
            "approval_closed"
        );
    }
}

#[test]
fn approvals_read_snapshots_paginate_history_and_derive_waiting_without_blocking_mail() {
    let t = crate::runtime::test_directory();
    let mut c = core(t.path());
    running(&c, "a");
    running(&c, "b");
    let first = ask(&mut c, "a", "first");
    let second = ask(&mut c, "a", "second");
    let third = ask(&mut c, "b", "third");
    let pending = call(&mut c, "human", "read", json!({"op":"approvals"})).unwrap();
    assert_eq!(pending["approvals"].as_array().unwrap().len(), 2);
    let filtered = call(
        &mut c,
        "human",
        "read",
        json!({"op":"approvals","agent":"a"}),
    )
    .unwrap();
    assert_eq!(filtered["approvals"][0]["id"], second);
    let mut ids = Vec::new();
    let mut after = Input::Missing;
    loop {
        let page = c
            .store()
            .approvals(&ApprovalsArgs {
                all: true.into(),
                after,
                limit: 1.into(),
                ..Default::default()
            })
            .unwrap();
        if page.approvals.is_empty() {
            break;
        }
        ids.push(page.approvals[0].id.clone());
        after = page.next_after.unwrap().into();
    }
    ids.sort();
    let mut expected = vec![first, second.clone(), third];
    expected.sort();
    assert_eq!(ids, expected);
    let status = call(&mut c, "human", "read", json!({"op":"status","target":"a"})).unwrap();
    assert_eq!(status["agent"]["waiting_on"], "human");
    assert_eq!(status["agent"]["approval_id"], second);
    let agents = call(&mut c, "human", "read", json!({"op":"agents"})).unwrap();
    assert_eq!(agents["agents"][0]["waiting_on"], "human");
    call(
        &mut c,
        "human",
        "peer",
        json!({"op":"send","target":"a","body":"unrelated"}),
    )
    .unwrap();
    assert!(c.store().next_message("a").unwrap().is_some());
    assert_eq!(c.store().pending_approval("a").unwrap(), Some(second));
}

#[test]
fn approvals_are_counted_and_removed_by_purge_and_gc() {
    for gc in [false, true] {
        let t = crate::runtime::test_directory();
        let mut c = core(t.path());
        let mut a = running(&c, "a");
        ask(&mut c, "a", "first");
        ask(&mut c, "a", "second");
        a.state = AgentState::Idle;
        a.conversation_clean = true;
        a.execution_id = None;
        a.turn_id = None;
        c.store().save(&a).unwrap();
        call(
            &mut c,
            "human",
            "end",
            json!({"op":"terminate","target":"a"}),
        )
        .unwrap();
        let mut stopped = c.store().agent("a").unwrap();
        stopped.state = AgentState::Stopped;
        stopped.control = None;
        c.store().save(&stopped).unwrap();
        let raw = if gc {
            json!({"op":"gc","before":crate::state::now()+1})
        } else {
            json!({"op":"purge","target":"a"})
        };
        let result = call(&mut c, "human", "delete", raw).unwrap();
        assert_eq!(
            result[if gc { "stats" } else { "counts" }]["approvals"],
            2,
            "{result}"
        );
        assert_eq!(count(&db(&t), "approvals"), 0);
    }
}

#[test]
fn approvals_gc_budget_includes_closed_history() {
    let t = crate::runtime::test_directory();
    let mut c = core(t.path());
    for id in ["a", "b"] {
        let mut a = agent(id);
        a.state = AgentState::Stopped;
        a.task_outcome = TaskOutcome::Completed;
        c.store().save(&a).unwrap();
    }
    let sql = db(&t);
    for agent in ["a", "b"] {
        sql.execute("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<60000) INSERT INTO approvals(id,agent,execution,turn,message,destructive,state,created_at,closed_at) SELECT ?1||i,?1,'e','t','old',0,'withdrawn',0,1 FROM n",[agent]).unwrap();
    }
    let page = call(
        &mut c,
        "human",
        "gc1",
        json!({"op":"gc","before":crate::state::now()+1}),
    )
    .unwrap();
    assert_eq!(page["stats"]["deleted_agents"], 1);
    assert_eq!(page["stats"]["approvals"], 60000);
    assert!(page["stats"]["work_rows"].as_u64().unwrap() >= 60000);
    assert_eq!(count(&sql, "approvals"), 60000);
    let page = call(
        &mut c,
        "human",
        "gc2",
        json!({"op":"gc","before":crate::state::now()+1,"after":page["next_after"]}),
    )
    .unwrap();
    assert_eq!(page["stats"]["deleted_agents"], 1);
    assert_eq!(count(&sql, "approvals"), 0);
}

#[test]
fn approvals_replaying_answer_does_not_close_new_question_and_limits_are_atomic() {
    let t = crate::runtime::test_directory();
    let mut c = core(t.path());
    running(&c, "a");
    let first = ask(&mut c, "a", "first");
    let answer = respond(&mut c, &first, "answer");
    let next = ask(&mut c, "a", "next");
    assert_eq!(respond(&mut c, &first, "answer"), answer);
    assert_eq!(c.store().pending_approval("a").unwrap(), Some(next.clone()));
    for (key, value) in [
        ("message", json!("x".repeat(16385))),
        ("title", json!("x".repeat(257))),
        ("destructive", json!("true")),
    ] {
        let mut raw = json!({"op":"ask","message":"Hi"});
        raw[key] = value;
        assert!(call(&mut c, "a", "bad", raw).is_err());
        assert_eq!(c.store().pending_approval("a").unwrap(), Some(next.clone()));
    }
    assert_eq!(
        call(
            &mut c,
            "human",
            "bad",
            json!({"op":"respond","approval_id":next,"action":"decline","reason":"x".repeat(61441)})
        )
        .unwrap_err()
        .code(),
        "out_of_range"
    );
    assert_eq!(c.store().pending_approval("a").unwrap(), Some(next.clone()));
    let response = call(
        &mut c,
        "human",
        "valid",
        json!({"op":"respond","approval_id":next,"action":"decline","reason":"x".repeat(61440)}),
    )
    .unwrap();
    let messages = c.store().messages("a", None, 200).unwrap();
    let body = messages["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == response["message_id"])
        .unwrap()["body"]
        .as_str()
        .unwrap();
    assert!(body.len() <= 65536);
}

fn approval_field_error(result: crate::error::Result<Value>, code: &str, field: &str) {
    let error = result.unwrap_err();
    let wire = serde_json::to_value(&error).unwrap();
    assert_eq!(wire["code"], code, "{wire}");
    assert_eq!(wire["field"], field, "{wire}");
    assert!(!wire.to_string().contains("PRIVATE"));
    assert!(!format!("{error:?}").contains("PRIVATE"));
    assert!(wire.get("received").is_none());
}

#[test]
fn approval_read_fields_validate_shape_and_ranges_without_echoing_cursors() {
    let temp = crate::runtime::test_directory();
    let c = core(temp.path());
    for (field, value, pointer, code) in [
        ("agent", json!(""), "/agent", "invalid_value"),
        ("all", json!("PRIVATE"), "/all", "invalid_value"),
        ("limit", json!(0), "/limit", "out_of_range"),
        ("limit", json!(201), "/limit", "out_of_range"),
        ("limit", json!(null), "/limit", "invalid_value"),
        ("after", json!([1, "PRIVATE"]), "/after", "invalid_value"),
        (
            "after",
            json!({"created_at":1,"id":"PRIVATE", "extra":true}),
            "/after",
            "invalid_value",
        ),
        (
            "after",
            json!({"created_at":u64::MAX,"id":"PRIVATE"}),
            "/after/created_at",
            "out_of_range",
        ),
        (
            "after",
            json!({"created_at":1,"id":""}),
            "/after/id",
            "invalid_value",
        ),
        (
            "after",
            json!({"created_at":1,"id":"é".repeat(65)}),
            "/after/id",
            "invalid_value",
        ),
    ] {
        let mut request = json!({"op":"approvals"});
        request[field] = value;
        approval_field_error(super::pagination::observe(&c, request), code, pointer);
    }
    for limit in [1, 200] {
        let request = json!({"op":"approvals","limit":limit,"after":{"created_at":i64::MAX,"id":"é".repeat(64)}});
        assert!(super::pagination::observe(&c, request).is_ok());
    }
    approval_field_error(
        super::pagination::observe(&c, json!({"op":"approval","approval_id":"PRIVATE"})),
        "invalid_uuid",
        "/approval_id",
    );
    assert_eq!(
        super::pagination::observe(
            &c,
            json!({"op":"approval","approval_id":"00000000-0000-4000-8000-000000000001"})
        )
        .unwrap_err()
        .code(),
        "unknown_approval"
    );
}

#[test]
fn approval_response_field_errors_do_not_mutate() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    running(&c, "a");
    let id = ask(&mut c, "a", "q");
    for (patch, code, field) in [
        (json!({}), "missing_argument", "/action"),
        (json!({"action":"PRIVATE"}), "invalid_value", "/action"),
        (
            json!({"action":"cancel","content":{}}),
            "conflicting_arguments",
            "/content",
        ),
        (
            json!({"action":"accept","content":{}}),
            "conflicting_arguments",
            "/content",
        ),
        (
            json!({"action":"cancel","reason":"PRIVATE"}),
            "conflicting_arguments",
            "/reason",
        ),
        (
            json!({"action":"decline","reason":7}),
            "invalid_value",
            "/reason",
        ),
        (
            json!({"action":"decline","reason":"x".repeat(61441)}),
            "out_of_range",
            "/reason",
        ),
    ] {
        let mut raw = json!({"op":"respond","approval_id":id});
        raw.as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        approval_field_error(call(&mut c, "human", "same-retry", raw), code, field);
        assert_eq!(
            c.store().approval(&id).unwrap().state,
            ApprovalState::Pending
        );
        assert_eq!(count(&db(&temp), "messages"), 0);
    }
}

#[test]
fn approval_response_caps_keep_corruption_closed_and_replay_precedence() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    running(&c, "a");
    let id = ask(&mut c, "a", "q");
    let form = json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]});
    db(&temp)
        .execute(
            "UPDATE approvals SET schema=?1 WHERE id=?2",
            rusqlite::params![form.to_string(), id],
        )
        .unwrap();
    approval_field_error(
        call(
            &mut c,
            "human",
            "same-retry",
            json!({"op":"respond","approval_id":id,"action":"accept"}),
        ),
        "missing_argument",
        "/content",
    );
    let oversized = json!({"op":"respond","approval_id":id,"action":"accept","content":{"text":"x".repeat(61440)}});
    approval_field_error(
        call(&mut c, "human", "same-retry", oversized.clone()),
        "out_of_range",
        "/content",
    );
    db(&temp)
        .execute("UPDATE approvals SET schema='{}' WHERE id=?1", [&id])
        .unwrap();
    assert_eq!(
        call(&mut c, "human", "same-retry", oversized)
            .unwrap_err()
            .code(),
        "database_error"
    );
    assert_eq!(count(&db(&temp), "messages"), 0);
    db(&temp)
        .execute(
            "UPDATE approvals SET schema=?1 WHERE id=?2",
            rusqlite::params![form.to_string(), id],
        )
        .unwrap();
    let accepted = json!({"op":"respond","approval_id":id,"action":"accept","content":{"text":"x".repeat(61429)}});
    let answer = call(&mut c, "human", "same-retry", accepted.clone()).unwrap();
    assert_eq!(
        call(&mut c, "human", "same-retry", accepted).unwrap(),
        answer
    );
    assert_eq!(count(&db(&temp), "messages"), 1);
    assert_eq!(
        call(
            &mut c,
            "human",
            "after-closed",
            json!({"op":"respond","approval_id":id,"action":"PRIVATE"})
        )
        .unwrap_err()
        .code(),
        "approval_closed"
    );
}
