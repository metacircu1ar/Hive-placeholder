use super::*;

fn stream(session: &str, turn: Option<&str>) -> Value {
    json!({"type":"stream_event","session_id":session,"parent_tool_use_id":null,
        "user_message_uuid":turn,"event":{"type":"message_start","message":{"model":"claude-sonnet-5"}}})
}

#[test]
fn partial_stream_never_acknowledges_input() {
    let mut f = Fixture::new();
    let outgoing = f
        .codec
        .send(outgoing::request(3, outgoing::turn_input(&f.id, "hello")))
        .unwrap();
    let turn = outgoing[0]["uuid"].as_str().unwrap();
    f.codec
        .receive(json!({"type":"system","subtype":"init","session_id":f.id,
        "model":"claude-sonnet-5","permissionMode":"default"}))
        .unwrap();
    assert!(
        f.codec
            .receive(stream(&f.id, Some(turn)))
            .unwrap()
            .is_empty()
    );
    assert!(!f.codec.buffered());
    f.codec
        .receive(json!({"type":"user","uuid":turn,"session_id":f.id,"isReplay":true}))
        .unwrap();
    assert_eq!(f.codec.pop().unwrap()["id"], 3);
    assert_eq!(f.codec.pop().unwrap()["method"], "turn/started");
    assert!(!f.codec.buffered());
    f.codec
        .receive(json!({"type":"result","session_id":f.id,
        "user_message_uuid":turn,"subtype":"success","is_error":false,
        "terminal_reason":"completed"}))
        .unwrap();
    assert_eq!(f.codec.pop().unwrap()["method"], "turn/completed");
    assert!(
        f.codec.quiescent(),
        "valid streaming must preserve confirmed shutdown"
    );
}

#[test]
fn partial_stream_rejects_wrong_scope_model_malformed_and_late_frames() {
    let mut f = Fixture::new();
    assert!(
        f.codec.receive(stream(&f.id, None)).is_err(),
        "no input submitted"
    );
    for variant in ["session", "parent", "turn", "model", "malformed", "late"] {
        let mut f = Fixture::new();
        let turn = f.turn();
        let mut frame = stream(&f.id, Some(&turn));
        match variant {
            "session" => frame["session_id"] = json!("other"),
            "parent" => frame["parent_tool_use_id"] = json!("child-tool"),
            "turn" => frame["user_message_uuid"] = json!("old-turn"),
            "model" => frame["event"]["message"]["model"] = json!("other-model"),
            "malformed" => frame["event"] = json!({"type":"message_start","message":{}}),
            "late" => {
                f.codec
                    .receive(json!({"type":"result","session_id":f.id,
                    "user_message_uuid":turn,"subtype":"success","is_error":false,
                    "terminal_reason":"completed"}))
                    .unwrap();
                f.codec.pop();
            }
            _ => unreachable!(),
        }
        assert!(f.codec.receive(frame).is_err(), "{variant}");
    }
}

#[test]
fn partial_stream_after_native_result_is_rejected_while_shell_cleanup_is_pending() {
    let mut f = Fixture::new();
    let turn = f.turn();
    let token = f.codec.guard.expect().unwrap();
    f.codec
        .receive(json!({"type":"result","session_id":f.id,
            "user_message_uuid":turn,"subtype":"success","is_error":false,
            "terminal_reason":"completed"}))
        .unwrap();
    assert!(
        !f.codec.buffered(),
        "completion is waiting for shell cleanup"
    );
    assert!(f.codec.receive(stream(&f.id, Some(&turn))).is_err());
    f.codec.guard.revoke(&token);
}
