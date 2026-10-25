use super::{
    corpus_support::{events, execution},
    dispatch_tool,
};
use crate::runtime::{
    provider::protocol::IncomingFrame,
    test_support::{RuntimeFixture, agent},
};
use serde_json::json;

#[tokio::test]
async fn hosted_attachment_dispatch_captures_once_and_replays_after_source_removal() {
    let fixture = RuntimeFixture::new();
    let agent = agent();
    let source_dir = crate::runtime::test_directory();
    let source = source_dir.path().join("review.txt");
    std::fs::write(&source, "Review evidence").unwrap();
    let frame = IncomingFrame::parse(json!({"params":{
        "tool":"hive_send","turnId":"turn","arguments":{
            "target":"agent","body":"Review this file","attachments":[source],"request_id":"attachment-1"
        }
    }}));
    let result = dispatch_tool(execution(&fixture, &agent), frame.params())
        .await
        .unwrap()
        .unwrap();
    std::fs::remove_file(&source).unwrap();
    let replay = dispatch_tool(execution(&fixture, &agent), frame.params())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay, result);
    let id = result["message_id"].as_str().unwrap().to_owned();
    let attachments = fixture
        .hub
        .with(move |core| core.store().attachments(&id))
        .await
        .unwrap();
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].filename, "review.txt");
    assert_eq!(attachments[0].bytes, 15);
    match &attachments[0].source {
        crate::runtime::attachments::Source::Snapshot { path, .. } => {
            assert_eq!(std::fs::read_to_string(path).unwrap(), "Review evidence");
        }
        other @ crate::runtime::attachments::Source::LivePath { .. } => {
            panic!("expected snapshot: {other:?}")
        }
    }
    fixture.finish();
}
#[tokio::test]
async fn mutating_tools_persist_explicit_replay_id_before_call_id() {
    let fixture = RuntimeFixture::new();
    let agent = agent();
    let params = json!({"tool":"hive_complete","turnId":"turn","callId":"different-call",
        "arguments":{"body":"opaque completion","request_id":"explicit-id"}});
    let frame = IncomingFrame::parse(json!({"params":params}));
    let result = dispatch_tool(execution(&fixture, &agent), frame.params())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        dispatch_tool(execution(&fixture, &agent), frame.params())
            .await
            .unwrap()
            .unwrap(),
        result
    );
    let fingerprint = json!({"op":"complete","body":"opaque completion"}).to_string();
    let saved = fixture
        .hub
        .with(move |core| {
            Ok((
                core.store().prior("agent", "explicit-id", &fingerprint)?,
                core.store()
                    .prior("agent", "different-call", &fingerprint)?,
                core.store().prior("agent", "unknown", &fingerprint)?,
                core.store().agent("agent")?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(saved.0, Some(result));
    assert!(saved.1.is_none());
    assert!(saved.2.is_none());
    assert_eq!(
        saved.3.completion_pending.as_deref(),
        Some("opaque completion")
    );
    assert_eq!(
        events(&fixture).await["events"].as_array().unwrap().len(),
        1
    );
    for call_id in [
        None,
        Some(json!("fallback-call")),
        Some(json!(null)),
        Some(json!(42)),
    ] {
        let mut params = json!({"tool":"hive_complete","turnId":"turn",
            "arguments":{"body":"must not mutate"}});
        if let Some(call_id) = call_id {
            params["callId"] = call_id;
        }
        let frame = IncomingFrame::parse(json!({"params":params}));
        crate::test_support::assert_code!(
            dispatch_tool(execution(&fixture, &agent), frame.params())
                .await
                .unwrap(),
            "missing_argument"
        );
    }
    let connection =
        rusqlite::Connection::open(fixture.temp.path().join("runtime.sqlite")).unwrap();
    let ids: Vec<String> = connection
        .prepare("SELECT id FROM requests WHERE sender='agent'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(ids, vec!["explicit-id"]);
    drop(connection);
    fixture.finish();
}

#[tokio::test]
async fn semantic_input_errors_keep_hosted_tool_examples() {
    let fixture = RuntimeFixture::new();
    let agent = agent();
    // Zero is an unsigned integer; the page bound is checked by the consumer.
    let frame = IncomingFrame::parse(
        json!({"params":{"tool":"hive_read","turnId":"turn","arguments":{
        "target":"agent","limit":0}}}),
    );
    let error = dispatch_tool(execution(&fixture, &agent), frame.params())
        .await
        .unwrap()
        .unwrap_err();
    let error = serde_json::to_value(error).unwrap();
    assert_eq!(error["code"], "out_of_range");
    assert_eq!(error["field"], "/limit");
    let example: serde_json::Value =
        serde_json::from_str(error["examples"][0].as_str().unwrap()).unwrap();
    assert_eq!(example["name"], "hive_read");
    assert!(example["arguments"].is_object());
    fixture.finish();
}
