//! Session identity checks, persisted provider payloads and tool validation boundaries.
use super::{
    corpus_support::{active, events, execution, replay_ids},
    dispatch_tool, finish_turn, handle,
};
use crate::error::Error;
use crate::runtime::provider::protocol::IncomingFrame;
use crate::runtime::{
    locks::MutexExt,
    provider::WorkerTransport,
    test_support::{RuntimeFixture, agent},
};
use serde_json::{Value, json};

#[tokio::test]
async fn completion_uses_session_thread_and_active_turn() {
    let fixture = RuntimeFixture::new();
    let mut transport = WorkerTransport::new(&fixture, Value::Null);
    let mut agent = agent();
    agent.thread_id = Some("other".into());
    let mut active = active(&fixture).await;
    for (thread, turn) in [("other", "turn"), ("thread", "other-turn")] {
        let frame = json!({"method":"turn/completed","params":{
            "threadId":thread,"turn":{"id":turn,"status":"completed"}}});
        handle(
            execution(&fixture, &agent),
            &mut transport.session,
            "thread",
            &mut active,
            IncomingFrame::parse(frame),
        )
        .await
        .unwrap();
        assert_eq!(active.as_ref().unwrap().turn_id, "turn");
        assert!(transport.sent.lock_or_panic().is_empty());
        assert_eq!(events(&fixture).await["events"], json!([]));
        assert!(replay_ids(&fixture).is_empty());
        let stored = fixture
            .hub
            .with(|core| {
                Ok((
                    core.store().agent("agent")?,
                    core.store().messages("agent", None, 200)?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(stored.0.state.as_str(), "running");
        assert_eq!(stored.0.turn_id.as_deref(), Some("turn"));
        assert_eq!(stored.1["messages"][0]["state"], "submitted");
    }
    handle(
        execution(&fixture, &agent),
        &mut transport.session,
        "thread",
        &mut active,
        IncomingFrame::parse(
            json!({"method":"turn/completed","params":{"threadId":"thread",
            "turn":{"id":"turn","status":"completed"}}}),
        ),
    )
    .await
    .unwrap();
    assert!(active.is_none());
    assert!(transport.sent.lock_or_panic().is_empty());
    assert!(replay_ids(&fixture).is_empty());
    let stored = events(&fixture).await;
    assert_eq!(stored["events"].as_array().unwrap().len(), 1);
    assert_eq!(stored["events"][0]["type"], "turn_finished");
    assert_eq!(
        stored["events"][0]["data"],
        json!({
        "turn_id":"turn","status":"completed","message_id":"message"})
    );
    let saved = fixture
        .hub
        .with(|core| {
            Ok((
                core.store().agent("agent")?,
                core.store().messages("agent", None, 200)?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(saved.0.state.as_str(), "idle");
    assert!(saved.0.turn_id.is_none());
    assert_eq!(saved.1["messages"][0]["state"], "completed");
    transport.finish().await;
    fixture.finish();
}

#[tokio::test]
async fn tool_mismatches_reply_without_dispatch() {
    let fixture = RuntimeFixture::new();
    let mut transport = WorkerTransport::new(&fixture, Value::Null);
    let mut agent = agent();
    agent.thread_id = Some("other".into());
    let mut active = active(&fixture).await;
    for (index, (thread, turn)) in [
        ("thread", Some("other-turn")),
        ("thread", None),
        ("other", Some("turn")),
    ]
    .into_iter()
    .enumerate()
    {
        let id = json!({"raw-request":index});
        let mut frame = json!({"id":id,"method":"item/tool/call","params":{
            "threadId":thread,"tool":"hive_complete","callId":"different-call",
            "arguments":{"body":"must not dispatch","request_id":format!("replay-{index}")}}});
        if let Some(turn) = turn {
            frame["params"]["turnId"] = json!(turn);
        }
        handle(
            execution(&fixture, &agent),
            &mut transport.session,
            "thread",
            &mut active,
            IncomingFrame::parse(frame),
        )
        .await
        .unwrap();
        let replies = transport.sent.lock_or_panic().clone();
        assert_eq!(replies.len(), index + 1);
        let reply = replies.last().unwrap();
        assert_eq!(reply["id"], id);
        assert_eq!(reply["result"]["success"], false);
        let content: Value =
            serde_json::from_str(reply["result"]["contentItems"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(content["error"]["code"], "unsupported_provider_request");
        assert_eq!(active.as_ref().unwrap().turn_id, "turn");
        assert_eq!(events(&fixture).await["events"], json!([]));
        assert!(replay_ids(&fixture).is_empty());
        let saved = fixture
            .hub
            .with(|core| core.store().agent("agent"))
            .await
            .unwrap();
        assert!(saved.completion_pending.is_none());
        assert_eq!(saved.turn_id.as_deref(), Some("turn"));
        assert_eq!(saved.state.as_str(), "running");
    }
    transport.finish().await;
    fixture.finish();
}

#[tokio::test]
async fn notice_mismatches_and_deltas_emit_no_events() {
    let fixture = RuntimeFixture::new();
    let mut transport = WorkerTransport::new(&fixture, Value::Null);
    let mut agent = agent();
    agent.thread_id = Some("other".into());
    let mut active = active(&fixture).await;
    for method in ["item/completed", "item/agentMessage/delta"] {
        for (thread, turn) in [("thread", "other-turn"), ("other", "turn")] {
            let frame = json!({"method":method,"params":{"threadId":thread,"turnId":turn,
                "item":{"type":"dynamicToolCall","arguments":{"opaque":true}},
                "delta":"must not become an event"}});
            handle(
                execution(&fixture, &agent),
                &mut transport.session,
                "thread",
                &mut active,
                IncomingFrame::parse(frame),
            )
            .await
            .unwrap();
            assert_eq!(active.as_ref().unwrap().turn_id, "turn");
            assert!(transport.sent.lock_or_panic().is_empty());
            assert_eq!(events(&fixture).await["events"], json!([]));
            assert!(replay_ids(&fixture).is_empty());
        }
    }
    handle(
        execution(&fixture, &agent),
        &mut transport.session,
        "thread",
        &mut active,
        IncomingFrame::parse(json!({"method":"item/agentMessage/delta","params":{
            "threadId":"thread","turnId":"turn","delta":"still telemetry"}})),
    )
    .await
    .unwrap();
    assert_eq!(active.as_ref().unwrap().turn_id, "turn");
    assert!(transport.sent.lock_or_panic().is_empty());
    assert_eq!(events(&fixture).await["events"], json!([]));
    assert!(replay_ids(&fixture).is_empty());
    transport.finish().await;
    fixture.finish();
}

#[tokio::test]
async fn notice_matches_preserve_each_handled_payload() {
    let fixture = RuntimeFixture::new();
    let mut transport = WorkerTransport::new(&fixture, Value::Null);
    let mut agent = agent();
    agent.thread_id = Some("other".into());
    let mut active = active(&fixture).await;
    let mut expected = Vec::new();
    for item in notice_items() {
        let data = if item["type"] == "agentMessage" {
            json!({"turn_id":"turn","text":item["text"]})
        } else {
            json!({"turn_id":"turn","item":item})
        };
        let kind = if item["type"] == "agentMessage" {
            "assistant_message"
        } else {
            "tool_completed"
        };
        handle(
            execution(&fixture, &agent),
            &mut transport.session,
            "thread",
            &mut active,
            IncomingFrame::parse(json!({"method":"item/completed","params":{
                "threadId":"thread","turnId":"turn","item":item}})),
        )
        .await
        .unwrap();
        expected.push((kind, data));
    }
    for (error, data) in notice_errors() {
        handle(
            execution(&fixture, &agent),
            &mut transport.session,
            "thread",
            &mut active,
            IncomingFrame::parse(json!({"method":"error","params":error})),
        )
        .await
        .unwrap();
        expected.push(("provider_error", data));
    }
    for rerouted in [
        json!({"threadId":"thread","turnId":"turn","model":{"opaque":true},
            "extra":[null,17]}),
        json!({"threadId":"thread","turnId":"turn","extra":[null,1],"model":{"opaque":true}}),
    ] {
        crate::test_support::assert_code!(
            handle(
                execution(&fixture, &agent),
                &mut transport.session,
                "thread",
                &mut active,
                IncomingFrame::parse(json!({"method":"model/rerouted","params":rerouted}))
            )
            .await,
            "model_rerouted"
        );
        expected.push(("model_rerouted", rerouted));
    }
    let stored = events(&fixture).await;
    assert_eq!(stored["events"].as_array().unwrap().len(), expected.len());
    for (event, (kind, data)) in stored["events"].as_array().unwrap().iter().zip(expected) {
        assert_eq!(event["type"], kind);
        assert_eq!(event["data"], data);
    }
    assert_eq!(active.as_ref().unwrap().turn_id, "turn");
    assert!(transport.sent.lock_or_panic().is_empty());
    assert!(replay_ids(&fixture).is_empty());
    transport.finish().await;
    fixture.finish();
}

#[tokio::test]
async fn nonstring_methods_and_unmatched_fields_are_ignored() {
    let fixture = RuntimeFixture::new();
    let mut transport = WorkerTransport::new(&fixture, Value::Null);
    let agent = agent();
    let mut active = active(&fixture).await;
    for frame in [
        json!({}),
        json!(null),
        json!([]),
        json!({"id":null,"method":null}),
        json!({"id":"id","method":false}),
        json!({"method":{},"params":null}),
        json!({"method":"turn/completed","params":null}),
        json!({"method":"turn/completed","params":{"threadId":3}}),
        json!({"method":"turn/completed","params":{"threadId":"thread","turn":{"id":false}}}),
        json!({"method":"item/completed","params":{"threadId":"thread","turnId":null}}),
        json!({"method":"future/method","params":{"threadId":"thread","turnId":"turn"}}),
    ] {
        handle(
            execution(&fixture, &agent),
            &mut transport.session,
            "thread",
            &mut active,
            IncomingFrame::parse(frame.clone()),
        )
        .await
        .unwrap();
        assert!(active.is_some(), "{frame}");
        assert!(transport.sent.lock_or_panic().is_empty(), "{frame}");
    }
    assert_eq!(events(&fixture).await["events"], json!([]));
    transport.finish().await;
    fixture.finish();
}

#[tokio::test]
async fn completion_status_maps_state_but_keeps_raw_text() {
    for (status, message_state, agent_state, error) in [
        (Some(json!("completed")), "completed", "idle", None),
        (Some(json!("interrupted")), "interrupted", "idle", None),
        (
            Some(json!("failed")),
            "failed",
            "failed",
            Some("turn_failed"),
        ),
        (
            Some(json!("inProgress")),
            "failed",
            "failed",
            Some("turn_failed"),
        ),
        (
            Some(json!("futureStatus")),
            "failed",
            "failed",
            Some("turn_failed"),
        ),
        (Some(json!(null)), "failed", "failed", Some("turn_failed")),
        (Some(json!(17)), "failed", "failed", Some("turn_failed")),
        (
            Some(json!({"raw":true})),
            "failed",
            "failed",
            Some("turn_failed"),
        ),
        (None, "failed", "failed", Some("turn_failed")),
    ] {
        let fixture = RuntimeFixture::new();
        let agent = agent();
        let mut active = active(&fixture).await;
        let mut params = json!({"threadId":"thread","turn":{"id":"turn","extra":[1,null]}});
        if let Some(status) = &status {
            params["turn"]["status"] = status.clone();
        }
        let frame = IncomingFrame::parse(json!({"params":params}));
        let result = finish_turn(execution(&fixture, &agent), &mut active, frame.params()).await;
        assert_eq!(result.as_ref().err().map(Error::code), error, "{params}");
        assert!(active.is_none());
        let saved = fixture
            .hub
            .with(|core| {
                Ok((
                    core.store().agent("agent")?,
                    core.store().messages("agent", None, 200)?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(saved.0.state.as_str(), agent_state);
        assert!(saved.0.turn_id.is_none());
        assert_eq!(saved.1["messages"][0]["state"], message_state);
        let status_text = status.as_ref().and_then(Value::as_str).unwrap_or("unknown");
        let stored = events(&fixture).await;
        assert_eq!(stored["events"][0]["type"], "turn_finished");
        assert_eq!(
            stored["events"][0]["data"],
            json!({
            "turn_id":"turn","status":status_text,"message_id":"message"})
        );
        fixture.finish();
    }
}

#[tokio::test]
async fn notice_payloads_without_matching_identity_emit_no_events() {
    let fixture = RuntimeFixture::new();
    let mut transport = WorkerTransport::new(&fixture, Value::Null);
    let agent = agent();
    let mut active = active(&fixture).await;
    let cases = [
        (
            "item/completed",
            json!({"turnId":{"opaque":true},"item":{
            "type":"agentMessage","text":["raw",null,2]}}),
        ),
        (
            "item/completed",
            json!({"turnId":null,"item":{"type":"commandExecution",
            "extra":{"raw":[true,null]}}}),
        ),
        (
            "item/completed",
            json!({"turnId":"turn","item":{"type":"fileChange","opaque":5}}),
        ),
        (
            "item/completed",
            json!({"turnId":"turn","item":{"type":"dynamicToolCall",
            "arguments":{"deep":[null]},"contentItems":[{"text":"secret opaque"}]}}),
        ),
        (
            "error",
            json!({"turnId":17,"willRetry":{"opaque":false},"private":"redact"}),
        ),
        (
            "model/rerouted",
            json!({"turnId":"turn","extra":[null,1],"model":{"opaque":true}}),
        ),
    ];
    let invalid_items = [
        Value::Null,
        json!({}),
        json!({"item":null}),
        json!({"item":{"type":17}}),
        json!({"item":{"type":"future"}}),
    ];
    for (method, params) in cases.into_iter().chain(
        invalid_items
            .into_iter()
            .map(|params| ("item/completed", params)),
    ) {
        let mut frames = vec![json!({"method":method,"params":params})];
        if params.is_object() && params["turnId"].as_str().is_none() {
            // Also reach the turn/item checks with the correct thread identity.
            let mut addressed = frames[0].clone();
            addressed["params"]["threadId"] = json!("thread");
            if params.get("turnId").is_none() {
                addressed["params"]["turnId"] = json!("turn");
            }
            frames.push(addressed);
        }
        for raw in frames {
            let frame = IncomingFrame::parse(raw.clone());
            assert_eq!(frame.raw(), &raw);
            handle(
                execution(&fixture, &agent),
                &mut transport.session,
                "thread",
                &mut active,
                frame,
            )
            .await
            .unwrap();
            assert_eq!(active.as_ref().unwrap().turn_id, "turn");
            assert!(transport.sent.lock_or_panic().is_empty());
            assert_eq!(events(&fixture).await["events"], json!([]));
            assert!(replay_ids(&fixture).is_empty());
        }
    }
    transport.finish().await;
    fixture.finish();
}

#[tokio::test]
async fn tool_outer_and_inner_errors_keep_validation_order() {
    let fixture = RuntimeFixture::new();
    let agent = agent();
    for params in [
        Value::Null,
        json!({}),
        json!({"tool":null}),
        json!({"tool":false}),
        json!({"tool":""}),
    ] {
        let frame = IncomingFrame::parse(json!({"params":params}));
        crate::test_support::assert_code!(
            dispatch_tool(execution(&fixture, &agent), frame.params()).await,
            "invalid_request"
        );
    }
    let params = json!({"tool":"native_tool",
        "arguments":null});
    let frame = IncomingFrame::parse(json!({"params":params}));
    crate::test_support::assert_code!(
        dispatch_tool(execution(&fixture, &agent), frame.params()).await,
        "unsupported_tool"
    );
    for (params, code) in [
        (
            json!({"tool":"hive_unknown","arguments":{}}),
            "unknown_argument",
        ),
        (
            json!({"tool":"hive_agents","arguments":[]}),
            "invalid_value",
        ),
        (
            json!({"tool":"hive_complete","arguments":{
            "body":"wrong","request_id":"wrong","target":"someone-else"}}),
            "unknown_argument",
        ),
    ] {
        let frame = IncomingFrame::parse(json!({"params":params}));
        crate::test_support::assert_code!(
            dispatch_tool(execution(&fixture, &agent), frame.params())
                .await
                .unwrap(),
            code
        );
    }
    let params = json!({"tool":"hive_agents","turnId":"turn","callId":"fallback-id",
        "arguments":{},"extra":[null,{"opaque":true}]});
    let frame = IncomingFrame::parse(json!({"params":params}));
    assert!(
        dispatch_tool(execution(&fixture, &agent), frame.params())
            .await
            .unwrap()
            .is_ok()
    );
    assert_eq!(params["arguments"], json!({}));
    for turn in [Value::Null, json!(3), json!("different")] {
        let mut stale = params.clone();
        stale["turnId"] = turn;
        let frame = IncomingFrame::parse(json!({"params":stale}));
        crate::test_support::assert_code!(
            dispatch_tool(execution(&fixture, &agent), frame.params())
                .await
                .unwrap(),
            "stale_execution"
        );
    }
    fixture.finish();
}

#[tokio::test]
async fn request_ids_remain_raw_and_unsupported_input_replies_first() {
    let fixture = RuntimeFixture::new();
    let mut transport = WorkerTransport::new(&fixture, Value::Null);
    let agent = agent();
    let mut active = active(&fixture).await;
    for id in [
        json!(null),
        json!("tool-id"),
        json!(3.5),
        json!(-1),
        json!(true),
        json!({"raw":1}),
    ] {
        let frame = json!({"id":id,"method":"item/tool/call","params":{
            "threadId":"thread","turnId":"turn","tool":"hive_agents","arguments":[]}});
        handle(
            execution(&fixture, &agent),
            &mut transport.session,
            "thread",
            &mut active,
            IncomingFrame::parse(frame),
        )
        .await
        .unwrap();
        let sent = transport.sent.lock_or_panic().last().unwrap().clone();
        assert_eq!(sent["id"], id);
        assert_eq!(sent["result"]["success"], false);
        let content: Value =
            serde_json::from_str(sent["result"]["contentItems"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(content["error"]["code"], "invalid_value");
        assert_eq!(content["error"]["field"], "/");
        assert!(content["error"]["examples"].is_array());
    }
    let count = transport.sent.lock_or_panic().len();
    let malformed = json!({"id":"abort","method":"item/tool/call","params":{
        "threadId":"thread","turnId":"turn","tool":null}});
    crate::test_support::assert_code!(
        handle(
            execution(&fixture, &agent),
            &mut transport.session,
            "thread",
            &mut active,
            IncomingFrame::parse(malformed)
        )
        .await,
        "invalid_request"
    );
    assert_eq!(transport.sent.lock_or_panic().len(), count);
    let input = json!({"id":null,"method":"item/tool/requestUserInput","params":{
        "threadId":"thread","turnId":"turn"}});
    crate::test_support::assert_code!(
        handle(
            execution(&fixture, &agent),
            &mut transport.session,
            "thread",
            &mut active,
            IncomingFrame::parse(input)
        )
        .await,
        "provider_input_required"
    );
    let sent = transport.sent.lock_or_panic().last().unwrap().clone();
    assert_eq!(sent["id"], Value::Null);
    assert_eq!(transport.sent.lock_or_panic().len(), count + 1);
    let content: Value =
        serde_json::from_str(sent["result"]["contentItems"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(content["error"]["code"], "unsupported_provider_request");
    transport.finish().await;
    fixture.finish();
}

fn notice_items() -> [Value; 8] {
    [
        json!({"type":"agentMessage","text":["opaque",null]}),
        json!({"type":"commandExecution","opaque":[1,null]}),
        json!({"type":"fileChange","opaque":{"kept":true}}),
        json!({"type":"dynamicToolCall","arguments":{"raw":17}}),
        json!({"type":"agentMessage","text":["raw",null,2]}),
        json!({"type":"commandExecution","extra":{"raw":[true,null]}}),
        json!({"type":"fileChange","opaque":5}),
        json!({"type":"dynamicToolCall","arguments":{"deep":[null]},
            "contentItems":[{"text":"secret opaque"}]}),
    ]
}

fn notice_errors() -> [(Value, Value); 2] {
    [
        (
            json!({"threadId":"thread","turnId":"turn","willRetry":{"raw":true},
                "private":"redacted"}),
            json!({"turn_id":"turn","will_retry":{"raw":true}}),
        ),
        (
            json!({"threadId":"thread","turnId":"turn","willRetry":{"opaque":false},
                "private":"redact"}),
            json!({"turn_id":"turn","will_retry":{"opaque":false}}),
        ),
    ]
}
