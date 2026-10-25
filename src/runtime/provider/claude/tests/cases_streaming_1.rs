//! Streaming regressions.
use super::*;

#[test]
fn full_access_claude_disables_native_confinement_but_keeps_delegation_denied() {
    let launcher = crate::launch::test_cache().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let config = SessionConfig {
        sandbox: Sandbox::FullAccess,
        ..SessionConfig::for_test(
            Uuid::new_v4().to_string(),
            dir.path().canonicalize().unwrap(),
        )
    };
    let mut command = Command::new("claude");
    let _guard = config.configure(&mut command, &launcher).unwrap();
    let args: Vec<_> = command.get_args().collect();
    let settings: Value = serde_json::from_str(
        args.windows(2)
            .find(|pair| pair[0] == "--settings")
            .unwrap()[1]
            .to_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(settings["sandbox"]["enabled"], false);
    assert_eq!(settings["sandbox"]["allowUnsandboxedCommands"], true);
    assert!(settings["sandbox"].get("filesystem").is_none());
    assert!(settings["sandbox"].get("network").is_none());
    assert_eq!(
        settings["permissions"]["deny"],
        json!([
            "Agent",
            "Task",
            "TaskOutput",
            "TaskStop",
            "CronCreate",
            "CronDelete",
            "CronList"
        ])
    );
    assert!(config.writable("/somewhere/outside/workspace/.git/config"));
    assert!(config.writable("relative.txt"));
}

#[test]
fn native_replayed_control_replies_are_exact_echoes_not_new_acknowledgements() {
    let mut f = Fixture::new();
    let replies = f.codec.receive(json!({"type":"control_request","request_id":"native-1","request":{
        "subtype":"mcp_message","server_name":"hive","message":{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}}})).unwrap();
    assert!(f.codec.receive(replies[0].clone()).unwrap().is_empty());
    assert!(f.codec.pop().is_none());
    assert!(f.codec.receive(replies[0].clone()).is_err());
    let replies = f.codec.receive(json!({"type":"control_request","request_id":"native-2","request":{
        "subtype":"mcp_message","server_name":"hive","message":{"jsonrpc":"2.0","method":"notifications/initialized"}}})).unwrap();
    let mut forged = replies[0].clone();
    forged["response"]["response"] = json!({"different":true});
    assert!(f.codec.receive(forged).is_err());
    assert!(f.codec.receive(replies[0].clone()).unwrap().is_empty());
}

#[test]
fn native_terminal_shapes_bind_only_to_our_acknowledged_turn() {
    for (subtype, error, reason, interrupt, expected) in [
        ("success", false, Some("completed"), false, "completed"),
        ("success", true, Some("completed"), false, "failed"),
        ("error_max_turns", true, Some("max_turns"), false, "failed"),
        ("error_during_execution", true, None, true, "interrupted"),
        (
            "success",
            false,
            Some("aborted_streaming"),
            false,
            "interrupted",
        ),
        (
            "success",
            false,
            Some("aborted_tools"),
            false,
            "interrupted",
        ),
        ("success", false, Some("completed"), true, "completed"),
    ] {
        let mut f = Fixture::new();
        let turn = f.turn();
        let control = interrupt.then(|| {
            f.codec
                .send(outgoing::request(4, outgoing::interrupt(&f.id, &turn)))
                .unwrap()
        });
        f.codec.receive(json!({"type":"result","session_id":f.id,"subtype":subtype,"is_error":error,"terminal_reason":reason})).unwrap();
        assert_eq!(f.codec.pop().unwrap()["params"]["turn"]["status"], expected);
        if let Some(control) = control {
            f.codec.receive(json!({"type":"control_response","response":{"subtype":"success","request_id":control[0]["request_id"]}})).unwrap();
        }
        assert!(f.codec.quiescent());
    }
    for extra in [
        json!({"user_message_uuid":"wrong"}),
        json!({"origin":{"kind":"peer"}}),
        json!({"terminal_reason":"background_requested"}),
    ] {
        let mut f = Fixture::new();
        f.turn();
        let mut result = json!({"type":"result","session_id":f.id,"subtype":"success","is_error":false,"terminal_reason":"completed"});
        result
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(f.codec.receive(result).is_err());
        assert!(!f.codec.quiescent());
    }
}

#[test]
fn sealed_guard_denies_late_tools_without_creating_obligations() {
    let mut f = Fixture::new();
    let turn = f.turn();
    f.codec.guard.seal();
    assert_eq!(
        f.hook("Bash", json!({"command":"echo forbidden"}))["permissionDecision"],
        "deny"
    );
    assert_eq!(f.permission(json!({"command":"echo forbidden"})), "deny");
    assert!(f.codec.guard.clean());
    // Completing the turn proves no tool obligation was inserted.
    f.codec.receive(json!({"type":"result","session_id":f.id,"subtype":"success","is_error":false,"terminal_reason":"completed","user_message_uuid":turn})).unwrap();
    assert!(f.codec.quiescent());
}

#[test]
fn shell_permission_without_native_id_requires_exact_unique_unspent_envelope() {
    let mut f = Fixture::new();
    f.turn();
    let input = f.hook("Bash", json!({"command":"echo hi"}))["updatedInput"].clone();
    for expected in ["allow", "deny"] {
        let reply = f
            .codec
            .receive(json!({"type":"control_request","request_id":"missing-id",
            "request":{"subtype":"can_use_tool","tool_name":"Bash","input":input}}))
            .unwrap();
        assert_eq!(reply[0]["response"]["response"]["behavior"], expected);
    }
}
