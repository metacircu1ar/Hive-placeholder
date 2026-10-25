use super::*;

fn input(store: &Store, a: &mut Agent, id: &str) -> Message {
    store.enqueue(id, "human", &a.id, "input").unwrap();
    store.track_call(id).unwrap();
    let message = store.next_message(&a.id).unwrap().unwrap();
    assert_eq!(message.id, id);
    a.execution_id = Some("execution".into());
    a.thread_id = Some("conversation".into());
    a.turn_id = Some(format!("turn-{id}"));
    store.save(a).unwrap();
    store
        .message_state(&message, MessageState::Submitted)
        .unwrap();
    store
        .turn_started(
            a,
            crate::runtime::api::events::TurnStarted {
                attachments: &[],
                turn_id: a.turn_id.as_deref().unwrap(),
                message_id: id,
                sender: "human",
                selection: crate::runtime::api::events::TurnSelection {
                    model: &a.model,
                    effort: &a.effort,
                    options_revision: a.options_revision,
                    selection_verification: "requested_unverified",
                },
            },
        )
        .unwrap();
    message
}

fn output(store: &Store, a: &Agent, message: &str, text: &str, phase: &str) {
    store
        .assistant_output(
            a,
            Some(message),
            json!({"turn_id":a.turn_id,"text":text,"phase":phase}),
            true,
        )
        .unwrap();
}

#[test]
fn call_reply_is_bound_to_the_message_and_terminal_output_is_immutable() {
    let tmp = crate::runtime::test_directory();
    let store = Store::open(&tmp.path().join("db")).unwrap();
    let mut a = agent("a", None);
    store.save(&a).unwrap();
    let first = input(&store, &mut a, "first");
    output(&store, &a, "first", "progress", "commentary");
    let bound = a.clone();
    a.execution_id = Some("wrong-execution".into());
    output(&store, &a, "first", "wrong execution", "final_answer");
    a = bound.clone();
    a.turn_id = Some("wrong-turn".into());
    output(&store, &a, "first", "wrong turn", "final_answer");
    a = bound;
    assert_eq!(
        store.message_result("first").unwrap().output.unwrap()["text"],
        "progress"
    );
    output(&store, &a, "first", "answer", "final_answer");
    output(&store, &a, "first", "later commentary", "commentary");
    assert_eq!(
        store.message_result("first").unwrap().output.unwrap()["text"],
        "answer"
    );
    output(&store, &a, "first", "final answer", "final_answer");
    store
        .message_state(&first, MessageState::Completed)
        .unwrap();
    output(
        &store,
        &a,
        "first",
        "late invalid overwrite",
        "final_answer",
    );
    let second = input(&store, &mut a, "second");
    output(&store, &a, "second", "other turn", "unknown_future_phase");
    output(&store, &a, "first", "wrong binding", "final_answer");
    store.message_state(&second, MessageState::Failed).unwrap();
    a.state = AgentState::Failed;
    store.save(&a).unwrap();
    let reply = store.message_result("first").unwrap();
    assert_eq!(reply.outcome, api::CallOutcome::Replied);
    assert_eq!(reply.output.unwrap()["text"], "final answer");
    assert_eq!(reply.turn_id.as_deref(), Some("turn-first"));
    assert!(reply.events_after.is_some());
    assert!(!reply.output_partial);
    assert_eq!(
        store.message_result("second").unwrap().outcome,
        api::CallOutcome::Failed
    );
}

#[test]
fn call_empty_synthetic_and_unsuccessful_outputs_are_explicit() {
    let tmp = crate::runtime::test_directory();
    let store = Store::open(&tmp.path().join("db")).unwrap();
    let mut a = agent("a", None);
    store.save(&a).unwrap();
    for (index, state, expected) in [
        (0, MessageState::Completed, api::CallOutcome::Replied),
        (1, MessageState::Interrupted, api::CallOutcome::Cancelled),
        (2, MessageState::Undeliverable, api::CallOutcome::Cancelled),
        (3, MessageState::Uncertain, api::CallOutcome::Uncertain),
        (4, MessageState::Failed, api::CallOutcome::Failed),
    ] {
        let id = index.to_string();
        let message = input(&store, &mut a, &id);
        store
            .assistant_output(
                &a,
                Some(&id),
                json!({"turn_id":a.turn_id,"text":"synthetic"}),
                false,
            )
            .unwrap();
        store.message_state(&message, state).unwrap();
        let reply = store.message_result(&id).unwrap();
        assert_eq!(reply.outcome, expected);
        assert_eq!(reply.output, None);
        assert_eq!(reply.output_partial, state != MessageState::Completed);
    }
}

#[test]
fn call_artifact_is_published_once_and_survives_event_removal_and_reopen() {
    let tmp = crate::runtime::test_directory();
    let path = tmp.path().join("db");
    let store = Store::open(&path).unwrap();
    let mut a = agent("a", None);
    store.save(&a).unwrap();
    let message = input(&store, &mut a, "large");
    let text = "λ".repeat(20000);
    output(&store, &a, "large", &text, "final_answer");
    store
        .message_state(&message, MessageState::Completed)
        .unwrap();
    let original = store.message_result("large").unwrap().output.unwrap();
    let events = store.events("a", 0, 100).unwrap();
    assert_eq!(
        events["events"][1]["data"]["artifact"],
        original["artifact"]
    );
    assert_eq!(fs::read_dir(&store.artifacts_dir).unwrap().count(), 1);
    store.connection.execute("DELETE FROM events", []).unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.message_result("large").unwrap().output.unwrap(),
        original
    );
    let page = store
        .output(original["artifact"]["id"].as_str().unwrap(), 0, 65536)
        .unwrap();
    let content: Value = serde_json::from_str(page["text"].as_str().unwrap()).unwrap();
    assert_eq!(content["text"], text);
}

#[test]
fn call_queue_rank_respects_startup_redirect_and_active_input() {
    let tmp = crate::runtime::test_directory();
    let store = Store::open(&tmp.path().join("db")).unwrap();
    let mut a = agent("a", None);
    store.save(&a).unwrap();
    store
        .enqueue_kind("seed", "human", "a", "seed", true)
        .unwrap();
    store.enqueue("first", "human", "a", "first").unwrap();
    store.track_call("first").unwrap();
    store
        .enqueue_redirect("redirect", "human", &mut a, "redirect")
        .unwrap();
    store.track_call("redirect").unwrap();
    assert_eq!(store.message_result("first").unwrap().queued_ahead, Some(2));
    assert_eq!(
        store.message_result("redirect").unwrap().queued_ahead,
        Some(1)
    );
    let seed = store.next_message("a").unwrap().unwrap();
    store.message_state(&seed, MessageState::Submitted).unwrap();
    let reply = store.message_result("first").unwrap();
    assert_eq!(reply.queued_ahead, Some(1));
    assert!(reply.active_input);
    assert_eq!(
        store.message_result("seed").unwrap_err().code(),
        "unsupported_operation"
    );
    assert_eq!(
        store.message_result("missing").unwrap_err().code(),
        "unknown_message"
    );
}

#[test]
fn internal_turn_output_has_no_call_records_or_call_specific_writes() {
    static WRITES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    let tmp = crate::runtime::test_directory();
    let store = Store::open(&tmp.path().join("db")).unwrap();
    let a = agent("a", None);
    store.save(&a).unwrap();
    // Count even empty call-specific updates; trace callbacks cannot propagate panics.
    WRITES.store(0, std::sync::atomic::Ordering::Relaxed);
    store.connection.trace_v2(
        rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
        Some(|event| {
            if let rusqlite::trace::TraceEvent::Stmt(_, sql) = event
                && sql.contains("call_results")
            {
                WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }),
    );
    store
        .turn_started(
            &a,
            crate::runtime::api::events::TurnStarted {
                attachments: &[],
                turn_id: "t",
                message_id: "internal",
                sender: "parent",
                selection: crate::runtime::api::events::TurnSelection {
                    model: &a.model,
                    effort: &a.effort,
                    options_revision: a.options_revision,
                    selection_verification: "requested_unverified",
                },
            },
        )
        .unwrap();
    store
        .assistant_output(&a, None, json!({"turn_id":"t","text":"hello"}), true)
        .unwrap();
    store
        .connection
        .trace_v2(rusqlite::trace::TraceEventCodes::empty(), None);
    assert_eq!(WRITES.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[test]
fn call_queue_rank_caps_counts_and_uses_three_index_ranges() {
    let tmp = crate::runtime::test_directory();
    let store = Store::open(&tmp.path().join("db")).unwrap();
    store.save(&agent("a", None)).unwrap();
    store.begin().unwrap();
    for i in 0..10002 {
        let id = i.to_string();
        store.enqueue(&id, "human", "a", "queued").unwrap();
        if i >= 10000 {
            store.track_call(&id).unwrap();
        }
    }
    store.commit().unwrap();
    let exact = store.message_result("10000").unwrap();
    assert_eq!(exact.queued_ahead, Some(10000));
    assert!(exact.queue_count_exact);
    let capped = store.message_result("10001").unwrap();
    assert_eq!(capped.queued_ahead, Some(10000));
    assert!(!capped.queue_count_exact);
    let plan: Vec<String> = store
        .connection
        .prepare(&format!(
            "EXPLAIN QUERY PLAN {}",
            super::super::calls::queries::QUEUE_RANK
        ))
        .unwrap()
        .query_map(params!["a", false, false, 1], |r| r.get(3))
        .unwrap()
        .map(std::result::Result::unwrap)
        .collect();
    assert_eq!(
        plan.iter()
            .filter(
                |s| s.starts_with("SEARCH messages USING") && s.contains("INDEX messages_pending")
            )
            .count(),
        3,
        "{plan:?}"
    );
}

#[test]
fn uncertain_call_preserves_partial_text_and_binding_after_reopen() {
    let tmp = crate::runtime::test_directory();
    let path = tmp.path().join("db");
    let store = Store::open(&path).unwrap();
    let mut a = agent("a", None);
    store.save(&a).unwrap();
    input(&store, &mut a, "crashed");
    output(&store, &a, "crashed", "partial work", "commentary");
    store.uncertain(&a.id).unwrap();
    output(&store, &a, "crashed", "late final", "final_answer");
    drop(store);
    let store = Store::open(&path).unwrap();
    let reply = store.message_result("crashed").unwrap();
    assert_eq!(reply.outcome, api::CallOutcome::Uncertain);
    assert_eq!(reply.output.unwrap()["text"], "partial work");
    assert_eq!(reply.turn_id, a.turn_id);
    assert!(reply.output_partial);
    assert!(store.next_message("a").unwrap().is_none());
}

#[test]
fn call_retention_distinguishes_pending_fresh_expired_and_corrupt_rows() {
    let tmp = crate::runtime::test_directory();
    let root = tmp.path().join("state");
    state::directory(&root).unwrap();
    drop(State::new(root.clone()).unwrap().initialize().unwrap());
    state::directory(&root.join("runtime")).unwrap();
    let path = root.join("runtime/runtime.sqlite");
    let store = Store::open(&path).unwrap();
    store.save(&agent("a", None)).unwrap();
    let cases = [
        ("accepted", MessageState::Accepted),
        ("submitting", MessageState::Submitting),
        ("submitted", MessageState::Submitted),
        ("fresh", MessageState::Completed),
        ("old", MessageState::Completed),
        ("corrupt", MessageState::Completed),
    ];
    for (id, status) in cases {
        store.enqueue(id, "human", "a", "input").unwrap();
        store.track_call(id).unwrap();
        store
            .message_state(
                &Message {
                    has_attachments: false,
                    attachments: vec![],
                    joined: false,
                    id: id.into(),
                    sender: "human".into(),
                    recipient: "a".into(),
                    body: "input".into(),
                    state: MessageState::Accepted,
                },
                status,
            )
            .unwrap();
        store
            .connection
            .execute(
                "UPDATE call_results SET accepted_at=?1 WHERE message_id=?2",
                params![if id == "fresh" { 100 } else { 0 }, id],
            )
            .unwrap();
    }
    store
        .connection
        .execute("DELETE FROM call_results WHERE message_id='corrupt'", [])
        .unwrap();
    drop(store);
    let report = compact(&root, 100, 1).unwrap();
    assert_eq!(report.call_results_removed, 1);
    let store = Store::open(&path).unwrap();
    for id in ["accepted", "submitting", "submitted"] {
        assert_eq!(
            store.message_result(id).unwrap().outcome,
            api::CallOutcome::Pending
        );
    }
    assert_eq!(
        store.message_result("fresh").unwrap().outcome,
        api::CallOutcome::Replied
    );
    assert_eq!(
        store.message_result("old").unwrap_err().code(),
        "result_expired"
    );
    assert_eq!(
        store.message_result("corrupt").unwrap_err().code(),
        "database_error"
    );
}
