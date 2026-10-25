//! Recovery regressions.
use super::*;

#[test]
fn full_access_native_permission_prompts_allow_external_paths_without_bypassing_shell_receipts() {
    for sandbox in [
        Sandbox::ReadOnly,
        Sandbox::WorkspaceWrite,
        Sandbox::FullAccess,
    ] {
        let mut f =
            Fixture::with_extensions(None, Uuid::new_v4().to_string(), |c| c.sandbox = sandbox);
        f.turn();
        let frame = |tool, input| {
            json!({"type":"control_request","request_id":"prompt","request":{
            "subtype":"can_use_tool","tool_name":tool,"tool_use_id":"tool-1","input":input,
            "decision_reason":"other","blocked_path":"/outside/workspace/file"}})
        };
        let replies = f
            .codec
            .receive(frame(
                "Edit",
                json!({"file_path":"/outside/workspace/file"}),
            ))
            .unwrap();
        assert_eq!(
            replies[0]["response"]["response"]["behavior"],
            if sandbox == Sandbox::FullAccess {
                "allow"
            } else {
                "deny"
            }
        );
    }
    let mut f = Fixture::with_extensions(None, Uuid::new_v4().to_string(), |c| {
        c.sandbox = Sandbox::FullAccess;
    });
    f.turn();
    assert_eq!(
        f.permission(json!({"command":"git commit -am test"})),
        "deny"
    );
    let issued = f.hook("Bash", json!({"command":"git commit -am test"}));
    let response = f.codec.receive(json!({"type":"control_request","request_id":"prompt","request":{
        "subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"tool-1","input":issued["updatedInput"],
        "decision_reason":"other","blocked_path":"/outside/workspace/.git"}})).unwrap();
    assert_eq!(response[0]["response"]["response"]["behavior"], "allow");
    assert_eq!(f.permission(issued["updatedInput"].clone()), "deny");
}

#[test]
fn assistant_after_terminal_cannot_replace_output_while_shell_cleanup_is_pending() {
    let mut f = Fixture::new();
    let turn = f.turn();
    let token = f.codec.guard.expect().unwrap();
    for text in ["first", "second"] {
        f.assistant(
            Some("native"),
            "claude-sonnet-5",
            json!([{"type":"text","text":text}]),
        );
        f.codec.pop().unwrap();
    }
    f.codec
        .receive(
            json!({"type":"result","session_id":f.id,"subtype":"success",
        "is_error":false,"terminal_reason":"completed","user_message_uuid":turn}),
        )
        .unwrap();
    assert_eq!(
        f.codec.pop().unwrap()["params"]["item"]["text"],
        "first\nsecond"
    );
    assert!(f.codec.pop().is_none(), "shell guard must delay completion");
    assert!(!f.codec.quiescent());
    assert!(f.codec.receive(json!({"type":"assistant","session_id":f.id,"message":{
        "id":"late","model":"claude-sonnet-5","content":[{"type":"text","text":"late overwrite"}]}})).is_err());
    assert!(f.codec.pop().is_none());
    f.codec.guard.revoke(&token);
    f.codec.settle();
    assert_eq!(f.codec.pop().unwrap()["method"], "turn/completed");
    assert!(f.codec.pop().is_none());
}

#[test]
fn completion_wins_an_interrupt_race_and_empty_receipt_is_not_execution_proof() {
    for receipt in [json!({}), json!({"cancelled":null,"still_queued":null})] {
        let mut f = Fixture::new();
        let turn = f.turn();
        let control = f
            .codec
            .send(outgoing::request(4, outgoing::interrupt(&f.id, &turn)))
            .unwrap();
        f.codec.receive(json!({"type":"result","session_id":f.id,"subtype":"success","is_error":false,"terminal_reason":"completed","user_message_uuid":turn})).unwrap();
        assert_eq!(
            f.codec.pop().unwrap()["params"]["turn"]["status"],
            "completed"
        );
        f.codec.receive(json!({"type":"control_response","response":{"subtype":"success","request_id":control[0]["request_id"],"response":receipt}})).unwrap();
        assert_eq!(f.codec.pop().unwrap()["id"], 4);
        assert!(f.codec.quiescent());
        assert!(
            f.codec
                .send(outgoing::request(5, outgoing::interrupt(&f.id, &turn)))
                .unwrap()
                .is_empty()
        );
        assert!(f.codec.pop().unwrap()["error"].is_object());
    }
}

#[test]
fn preinput_cleanup_requires_no_execution_evidence_and_no_submitted_input() {
    for (output, exit, clean) in [
        (
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"unknown","response":{}}}"#,
            0,
            true,
        ),
        ("", 1, true),
        ("", 7, false),
        (r#"{"type":"system","subtype":"task_started"}"#, 0, false),
        ("malformed native output", 0, false),
        (
            concat!(
                "{\"type\":\"control_response\",\"response\":{\"subtype\":\"success\",\"request_id\":\"unknown1\"}}\n",
                "{\"type\":\"control_response\",\"response\":{\"subtype\":\"success\",\"request_id\":\"unknown2\"}}\n",
                "{\"type\":\"control_response\",\"response\":{\"subtype\":\"success\",\"request_id\":\"unknown3\"}}\n",
                "{\"type\":\"system\",\"subtype\":\"task_started\"}"
            ),
            0,
            false,
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let config = Arc::new(SessionConfig::for_test(
            Uuid::new_v4().to_string(),
            dir.path().canonicalize().unwrap(),
        ));
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "if [ -n \"$1\" ]; then printf '%s\\n' \"$1\"; fi; exit \"$2\"",
            "probe",
            output,
            &exit.to_string(),
        ]);
        let launcher = crate::launch::test_cache().unwrap();
        let mut rpc = super::super::super::rpc::Rpc::start_with(
            &launcher,
            command,
            super::super::super::Backend::Claude(config),
            vec![],
        )
        .unwrap();
        assert!(rpc.initialize().is_err());
        assert_eq!(
            rpc.close().confirmed,
            clean,
            "output={output:?}, exit={exit}"
        );
    }
    let mut f = Fixture::new();
    assert!(!f.codec.preinput_quiescent());
    f.codec.guard.seal();
    assert!(f.codec.preinput_quiescent());
    f.turn();
    assert!(!f.codec.preinput_quiescent());
}

#[test]
fn unknown_preinput_frames_veto_ordinary_and_exceptional_cleanup() {
    for frame in [
        json!({"type":"future_native_work"}),
        json!({"type":"system","subtype":"future_native_work"}),
    ] {
        let mut f = Fixture::new();
        assert!(f.codec.quiescent());
        assert!(f.codec.receive(frame).is_err());
        f.codec.guard.seal();
        assert!(!f.codec.quiescent());
        assert!(!f.codec.preinput_quiescent());
    }
}
