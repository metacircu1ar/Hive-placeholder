use super::*;
use serde_json::json;

fn frame(value: Value) -> IncomingFrame {
    IncomingFrame::parse(value)
}

#[test]
fn only_displayed_delta_families_are_intercepted() {
    for (method, kind) in [
        (AGENT_MESSAGE_DELTA, Some(DeltaKind::Shown(Kind::Text))),
        (
            COMMAND_OUTPUT_DELTA,
            Some(DeltaKind::Shown(Kind::ToolOutput)),
        ),
        (
            REASONING_SUMMARY_DELTA,
            Some(DeltaKind::Shown(Kind::Reasoning)),
        ),
        (RAW_REASONING_DELTA, Some(DeltaKind::Hidden)),
        ("item/completed", None),
        ("turn/started", None),
    ] {
        assert_eq!(delta_kind(&json!({"method": method, "params": {}})), kind);
    }
    assert_eq!(delta_kind(&json!({"id": 1, "result": {}})), None);
    let raw = json!({"method": AGENT_MESSAGE_DELTA, "params":
        {"threadId":"t","turnId":"u","itemId":"i","delta":"Hel"}});
    assert_eq!(
        delta(Kind::Text, &raw),
        Some(Delta {
            thread: "t".into(),
            event: Event {
                turn_id: Some("u".into()),
                item_id: Some("i".into()),
                text: Some("Hel".into()),
                ..Event::new(Kind::Text)
            },
        })
    );
    // Malformed deltas are ignored rather than failing the provider.
    for params in [
        json!({}),
        json!({"threadId": 1, "delta": "x"}),
        json!({"threadId": "t"}),
    ] {
        assert_eq!(
            delta(
                Kind::Text,
                &json!({"method": AGENT_MESSAGE_DELTA, "params": params})
            ),
            None
        );
    }
}

#[test]
fn queued_deltas_split_by_read_order_and_overflow_records_gaps() {
    let text = |thread: &str| Delta {
        thread: thread.into(),
        event: Event::new(Kind::Text),
    };
    let mut deltas = Deltas::default();
    for (seq, thread) in [(1, "a"), (2, "b"), (4, "c")] {
        deltas.push(seq, text(thread));
    }
    // Only deltas read before the next frame are ready; the rest wait for it.
    let ready = deltas.before(Some(3));
    let threads: Vec<_> = ready.deltas.iter().map(|d| d.thread.as_str()).collect();
    assert_eq!(threads, ["a", "b"]);
    assert!(ready.gaps.is_empty() && !ready.all_gaps);
    assert_eq!(deltas.before(None).deltas.len(), 1);
    for index in 0..(QUEUED_DELTAS + 3) {
        deltas.push(index as u64, text(&index.to_string()));
    }
    let ready = deltas.before(None);
    assert_eq!(ready.deltas.len(), QUEUED_DELTAS);
    assert_eq!(ready.deltas[0].thread, "3");
    let mut gaps = ready.gaps;
    gaps.sort();
    assert_eq!(gaps, ["0", "1", "2"]);
    assert!(!ready.all_gaps);
    // Past the remembered set, every thread resyncs.
    for index in 0..=(QUEUED_DELTAS + QUEUED_GAPS) {
        deltas.push(index as u64, text(&index.to_string()));
    }
    let ready = deltas.before(None);
    assert_eq!(ready.gaps.len(), QUEUED_GAPS);
    assert!(ready.all_gaps);
    let empty = deltas.before(None);
    assert!(empty.deltas.is_empty() && empty.gaps.is_empty() && !empty.all_gaps);
}

#[test]
fn lifecycle_frames_map_only_allowlisted_items() {
    let started = frame(json!({"method":"turn/started","params":
        {"threadId":"t","turn":{"id":"u","status":"inProgress"}}}));
    assert_eq!(
        event(&started),
        Some(Event {
            turn_id: Some("u".into()),
            status: Some("inProgress".into()),
            ..Event::new(Kind::TurnStarted)
        })
    );
    let completed = frame(json!({"method":"turn/completed","params":
        {"threadId":"t","turn":{"id":"u","status":"completed"}}}));
    assert_eq!(event(&completed).unwrap().kind, Some(Kind::TurnCompleted));
    let command = |method: &str, extra: Value| {
        let mut item = json!({"type":"commandExecution","id":"c","command":"cargo test",
            "cwd":"/w","commandActions":[],"status":"inProgress"});
        item.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        frame(json!({"method": method, "params": {"threadId":"t","turnId":"u","item": item}}))
    };
    let begun = event(&command("item/started", json!({}))).unwrap();
    assert_eq!(
        (
            begun.kind,
            begun.tool.as_deref(),
            begun.text.as_deref(),
            begun.status
        ),
        (
            Some(Kind::ToolStarted),
            Some("command"),
            Some("cargo test"),
            None
        )
    );
    let ended = event(&command(
        "item/completed",
        json!({"status":"failed","exitCode":101}),
    ))
    .unwrap();
    assert_eq!(
        (ended.kind, ended.text.as_deref(), ended.status.as_deref()),
        (Some(Kind::ToolCompleted), Some("exit 101"), Some("failed"))
    );
    let argv = event(&command(
        "item/started",
        json!({"command":["git","status"]}),
    ))
    .unwrap();
    assert_eq!(argv.text.as_deref(), Some("git status"));
    let files = frame(
        json!({"method":"item/started","params":{"threadId":"t","turnId":"u",
        "item":{"type":"fileChange","id":"f","status":"inProgress",
            "changes":[{"path":"a.rs"},{"path":"b.rs"}]}}}),
    );
    assert_eq!(event(&files).unwrap().text.as_deref(), Some("a.rs, b.rs"));
    let tool = frame(
        json!({"method":"item/started","params":{"threadId":"t","turnId":"u",
        "item":{"type":"dynamicToolCall","id":"d","tool":"hive_send","arguments":{},"status":"inProgress"}}}),
    );
    assert_eq!(event(&tool).unwrap().tool.as_deref(), Some("hive_send"));
    let mcp = frame(
        json!({"method":"item/started","params":{"threadId":"t","turnId":"u",
        "item":{"type":"mcpToolCall","id":"m","server":"docs","tool":"search","arguments":{},"status":"inProgress"}}}),
    );
    assert_eq!(event(&mcp).unwrap().tool.as_deref(), Some("docs/search"));
}

#[test]
fn echoes_assistant_text_and_unknown_items_are_not_live_items() {
    // User echoes, assistant text and unknown items are durable-only or dropped.
    for item in [
        json!({"type":"userMessage","id":"x","content":[{"type":"text","text":"a whole joined reply"}]}),
        json!({"type":"agentMessage","id":"x","text":"answer"}),
        json!({"type":"hookPrompt","id":"x","fragments":[]}),
        json!({"type":"somethingNew","id":"x"}),
    ] {
        for method in ["item/started", "item/completed"] {
            let value = frame(
                json!({"method": method, "params": {"threadId":"t","turnId":"u","item": item}}),
            );
            assert_eq!(event(&value), None, "{item}");
        }
    }
    assert_eq!(
        event(&frame(
            json!({"method":"model/rerouted","params":{"threadId":"t"}})
        )),
        None
    );
}
