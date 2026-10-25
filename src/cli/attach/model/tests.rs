use super::*;
use serde_json::json;

fn event(seq: u64, kind: &str, data: Value) -> Value {
    json!({"seq": seq, "agent_id": "a", "execution_id": null, "timestamp": 1, "type": kind, "data": data})
}

fn page(events: &[Value]) -> Value {
    let cursor = events
        .iter()
        .filter_map(|event| event["seq"].as_u64())
        .max()
        .unwrap_or(0);
    json!({"events": events, "cursor": cursor, "pruned_through": 0, "history_expired": false})
}

fn snapshot(provider: &str, events: &[Value]) -> Value {
    json!({
        "agent": {"id": "a", "state": "idle", "task_outcome": "open", "model": "m", "effort": "low",
                  "auth": "main", "profile": {"provider": provider}, "ended_at": null},
        "events": page(events), "queued": 0, "mail": 0, "children": 0,
    })
}

fn live(kind: &str, key: &str, text: &str) -> Value {
    json!({"seq": 0, "kind": kind, "turn_id": "t", "item_id": key, "text": text})
}

fn assistant_texts(model: &Model) -> Vec<(String, String)> {
    model
        .entries()
        .filter_map(|entry| match entry {
            Entry::Assistant { text, live, .. } => Some((text.clone(), live.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn unresolved_artifacts_preserve_live_answers_and_expose_output_references() {
    for provider in ["codex", "claude"] {
        let mut model = Model::from_snapshot(&snapshot(provider, &[]));
        model.apply_live(&[live("text", "answer", "readable answer")]);
        let missing = event(
            1,
            "assistant_message",
            json!({"item_id":"answer",
            "aggregation":"native_message", "artifact":{"id":"artifact-id"}}),
        );
        model.apply_page(&page(&[
            missing.clone(),
            event(2, "turn_finished", json!({"status":"completed"})),
        ]));
        assert!(
            assistant_texts(&model)
                .iter()
                .any(|(text, live)| text == "readable answer" || live == "readable answer")
        );
        let reopened = Model::from_snapshot(&snapshot(provider, &[missing]));
        assert!(reopened.entries().any(
            |entry| matches!(entry, Entry::Notice(text) if text.contains("hivectl output artifact-id"))
        ));
    }
}

#[test]
fn codex_live_items_give_way_to_durable_events_without_duplicates() {
    let first = [
        event(
            1,
            "message_accepted",
            json!({"sender": "human", "body": "hi"}),
        ),
        event(2, "turn_started", json!({"model": "m", "effort": "low"})),
        event(
            3,
            "assistant_message",
            json!({"item_id": "m0", "text": "hello"}),
        ),
        event(4, "turn_finished", json!({"status": "completed"})),
    ];
    let mut model = Model::from_snapshot(&snapshot("codex", &first));
    assert_eq!((model.cursor(), model.older()), (4, Some(1)));
    model.apply_live(&[
        live("turn_started", "", ""),
        live("tool_started", "c1", "make check"),
        live("tool_output", "c1", "ok\n"),
        live("tool_completed", "c1", "exit 0"),
        live("text", "m1", "streamed "),
        live("text", "m1", "answer"),
    ]);
    assert!(model.streaming);
    assert_eq!(
        assistant_texts(&model).last(),
        Some(&(String::new(), String::from("streamed answer")))
    );
    let second = [
        event(
            6,
            "tool_completed",
            json!({"item": {"type": "commandExecution", "id": "c1",
            "command": "make check", "status": "completed", "exitCode": 0}}),
        ),
        event(
            7,
            "assistant_message",
            json!({"item_id": "m1", "text": "streamed answer"}),
        ),
        event(8, "turn_finished", json!({"status": "completed"})),
    ];
    // A refresh can repeat rows already applied; they are skipped.
    model.apply_page(&page(&[&first[..], &second[..]].concat()));
    model.apply_page(&page(&second));
    assert!(!model.streaming);
    assert_eq!(model.cursor(), 8);
    let tools: Vec<_> = model
        .entries()
        .filter_map(|entry| match entry {
            Entry::Tool {
                summary,
                output,
                running,
                ..
            } => Some((summary.clone(), output.clone(), *running)),
            _ => None,
        })
        .collect();
    // The durable tool keeps the streamed output it lacks.
    assert_eq!(
        tools,
        [(String::from("make check"), String::from("ok\n"), false)]
    );
    assert_eq!(
        assistant_texts(&model),
        [
            (String::from("hello"), String::new()),
            (String::from("streamed answer"), String::new())
        ]
    );
}

#[test]
fn claude_blocks_append_never_shrink_and_the_aggregate_replaces_them() {
    let mut model = Model::from_snapshot(&snapshot("claude", &[]));
    assert!(model.header.claude);
    model.apply_live(&[live("text", "msg_1", "First block. Second block.")]);
    // The first stored block is shorter than the streamed text, which stays.
    model.apply_page(&page(&[event(
        1,
        "assistant_message",
        json!({"item_id": "msg_1", "text": "First block."}),
    )]));
    assert_eq!(
        assistant_texts(&model),
        [(
            String::from("First block."),
            String::from("First block. Second block.")
        )]
    );
    model.apply_page(&page(&[event(
        2,
        "assistant_message",
        json!({"item_id": "msg_1", "text": "Second block."}),
    )]));
    model.apply_page(&page(&[event(
        3,
        "assistant_message",
        json!({"item_id": "msg_1", "text": "First block.\n\nSecond block.",
               "aggregation": "native_message", "native_message_id": "msg_1"}),
    )]));
    let entries: Vec<_> = model.entries().cloned().collect();
    assert_eq!(
        entries,
        [Entry::Assistant {
            key: Some(String::from("msg_1")),
            text: String::from("First block.\n\nSecond block."),
            live: String::new(),
            aggregate: true,
        }]
    );
    // A single-block message has no aggregate; its block stands at turn end.
    model.apply_live(&[live("text", "msg_2", "Only.")]);
    model.apply_page(&page(&[
        event(
            4,
            "assistant_message",
            json!({"item_id": "msg_2", "text": "Only."}),
        ),
        event(5, "turn_finished", json!({"status": "completed"})),
    ]));
    assert_eq!(
        assistant_texts(&model).last(),
        Some(&(String::from("Only."), String::new()))
    );
}

#[test]
fn mcp_tools_close_with_the_turn_and_stored_tools_are_not_reopened() {
    let mut model = Model::from_snapshot(&snapshot("codex", &[]));
    model.apply_live(&[
        live("turn_started", "", ""),
        json!({"kind": "tool_started", "item_id": "x1", "tool": "hive/hive_send", "text": ""}),
    ]);
    model.apply_page(&page(&[event(
        1,
        "turn_finished",
        json!({"status": "completed"}),
    )]));
    let tools: Vec<_> = model
        .entries()
        .filter_map(|entry| match entry {
            Entry::Tool { tool, running, .. } => Some((tool.clone(), *running)),
            _ => None,
        })
        .collect();
    assert_eq!(tools, [(String::from("hive/hive_send"), false)]);
    model.apply_page(&page(&[event(
        2,
        "tool_completed",
        json!({"item": {"type": "commandExecution", "id": "c9", "command": "ls", "exitCode": 0}}),
    )]));
    model.apply_live(&[live("tool_started", "c9", "ls")]);
    let running = model
        .entries()
        .filter(|entry| matches!(entry, Entry::Tool { running: true, .. }))
        .count();
    assert_eq!(running, 0);
}

#[test]
fn resync_drops_partial_live_text_until_the_durable_message_arrives() {
    let mut model = Model::from_snapshot(&snapshot("codex", &[]));
    model.apply_live(&[live("text", "m1", "partial")]);
    model.resync();
    assert!(assistant_texts(&model).is_empty() && !model.streaming);
    model.apply_live(&[live("text", "m1", " more")]);
    model.apply_page(&page(&[event(
        1,
        "assistant_message",
        json!({"item_id": "m1", "text": "partial more"}),
    )]));
    assert_eq!(
        assistant_texts(&model),
        [(String::from("partial more"), String::new())]
    );
}

#[test]
fn history_pages_prepend_and_report_the_retained_start() {
    let newest = [event(
        9,
        "message_accepted",
        json!({"sender": "human", "body": "newest"}),
    )];
    let mut model = Model::from_snapshot(&snapshot("codex", &newest));
    assert_eq!(model.older(), Some(9));
    model.prepend(&json!({
        "events": [event(3, "message_accepted", json!({"sender": "human", "body": "older"}))],
        "older": null, "pruned_through": 2, "history_expired": true,
    }));
    let bodies: Vec<_> = model
        .entries()
        .filter_map(|entry| match entry {
            Entry::Input { body, .. } => Some(body.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(bodies, ["older", "newest"]);
    assert!(model.older().is_none() && model.history_expired());
}

#[test]
fn live_tails_and_the_transcript_are_bounded() {
    let mut model = Model::from_snapshot(&snapshot("codex", &[]));
    let chunk = "é".repeat(LIVE_TAIL / 4);
    for _ in 0..4 {
        model.apply_live(&[live("text", "m", &chunk), live("text", "m", "!")]);
    }
    let (_, streamed) = assistant_texts(&model).pop().unwrap();
    assert!(streamed.len() <= LIVE_TAIL && streamed.ends_with('!'));
    let events: Vec<_> = (1..=(ENTRY_LIMIT as u64 + 10))
        .map(|seq| {
            event(
                seq,
                "message_accepted",
                json!({"sender": "human", "body": "x"}),
            )
        })
        .collect();
    model.apply_page(&page(&events));
    assert!(model.entries().count() <= ENTRY_LIMIT + 1);
    // The first kept row is where older history resumes, but the model is full.
    assert!(model.older.is_some_and(|older| older > 1));
    assert!(model.older().is_none());
}

#[test]
fn header_reads_the_attach_snapshot() {
    let mut reply = snapshot("claude", &[]);
    reply["agent"]["state"] = json!("running");
    reply["agent"]["steering_supported"] = json!(true);
    reply["agent"]["waiting_on"] = json!("human");
    reply["agent"]["auth_decision"] = json!({"mode": "balanced", "reason": "least_loaded"});
    reply["children"] = json!(2);
    let header = Model::from_snapshot(&reply).header;
    assert!(header.running() && header.steering && header.claude && !header.ended);
    assert_eq!(
        (
            header.waiting.as_deref(),
            header.auth_decision.as_deref(),
            header.children
        ),
        (Some("human"), Some("least_loaded"), 2)
    );
}

#[test]
fn a_finished_turn_keeps_the_next_turns_live_entries() {
    let mut model = Model::from_snapshot(&snapshot("codex", &[]));
    let item = |kind: &str, turn: &str, key: &str, text: &str| json!({"kind": kind, "turn_id": turn, "item_id": key, "text": text});
    model.apply_live(&[
        item("tool_started", "t1", "c1", "ls"),
        item("text", "t2", "m2", "next turn text"),
        item("tool_started", "t2", "c2", "pwd"),
    ]);
    model.apply_page(&page(&[event(
        1,
        "turn_finished",
        json!({"turn_id": "t1", "status": "completed"}),
    )]));
    // t1's tool closed into the transcript; t2's entries still stream.
    assert!(model.streaming);
    model.apply_live(&[item("tool_output", "t2", "c2", "/w\n")]);
    model.apply_page(&page(&[event(
        2,
        "assistant_message",
        json!({"item_id": "m2", "text": "next turn text"}),
    )]));
    let texts = assistant_texts(&model);
    assert_eq!(texts, [(String::from("next turn text"), String::new())]);
    let outputs: Vec<_> = model
        .entries()
        .filter_map(|entry| match entry {
            Entry::Tool {
                summary,
                output,
                running,
                ..
            } => Some((summary.clone(), output.clone(), *running)),
            _ => None,
        })
        .collect();
    assert_eq!(
        outputs,
        [
            (String::from("ls"), String::new(), false),
            (String::from("pwd"), String::from("/w\n"), true)
        ]
    );
}

#[test]
fn the_live_area_is_bounded_and_a_full_transcript_says_so() {
    let mut model = Model::from_snapshot(&snapshot("codex", &[]));
    let items: Vec<_> = (0..LIVE_LIMIT + 20)
        .map(|index| json!({"kind": "reasoning", "turn_id": "t", "item_id": index.to_string(), "text": "x"}))
        .collect();
    model.apply_live(&items);
    assert_eq!(model.entries().count(), LIVE_LIMIT);
    let events: Vec<_> = (1..=(ENTRY_LIMIT as u64 + 1))
        .map(|seq| {
            event(
                seq,
                "message_accepted",
                json!({"sender": "human", "body": "x"}),
            )
        })
        .collect();
    let mut model = Model::from_snapshot(&snapshot("codex", &[]));
    model.apply_page(&page(&events));
    assert!(model.full() && model.older().is_none());
}

#[test]
fn a_stopped_turn_closes_its_live_entries_without_turn_finished() {
    let mut model = Model::from_snapshot(&snapshot("codex", &[]));
    let item = |kind: &str, turn: &str, key: &str, text: &str| json!({"kind": kind, "turn_id": turn, "item_id": key, "text": text});
    model.apply_live(&[
        item("turn_started", "t1", "", ""),
        item("tool_started", "t1", "c1", "make check"),
        item("text", "t1", "m1", "partial"),
    ]);
    model.apply_page(&page(&[event(1, "execution_stopped", json!({}))]));
    assert!(!model.streaming);
    let running = model
        .entries()
        .filter(|entry| matches!(entry, Entry::Tool { running: true, .. }))
        .count();
    assert_eq!(running, 0);
    // A later turn finishing leaves nothing of the stopped turn in the live area.
    model.apply_live(&[item("text", "t2", "m2", "next")]);
    model.apply_page(&page(&[
        event(
            2,
            "assistant_message",
            json!({"item_id": "m2", "text": "next"}),
        ),
        event(
            3,
            "turn_finished",
            json!({"turn_id": "t2", "status": "completed"}),
        ),
    ]));
    assert!(!model.streaming);
    let last = model.entries().last().cloned();
    assert_eq!(
        last,
        Some(Entry::TurnFinished {
            status: String::from("completed")
        })
    );
}

#[test]
fn a_child_spawn_notice_names_the_child_when_it_has_a_name() {
    let mut model = Model::from_snapshot(&json!({
        "agent": {"id": "a", "state": "idle", "task_outcome": "open", "ended_at": null},
        "events": {"events": [], "cursor": 0, "pruned_through": 0, "history_expired": false},
        "queued": 0, "mail": 0, "children": 0,
    }));
    model.child_spawned("child-1", Some("diff check"), Some("reviewer"));
    model.child_spawned("child-2", None, None);
    let notices: Vec<&str> = model
        .entries()
        .filter_map(|entry| match entry {
            Entry::Notice(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        notices,
        [
            "spawned child child-1 \"diff check\" (reviewer)",
            "spawned child child-2"
        ]
    );
}
