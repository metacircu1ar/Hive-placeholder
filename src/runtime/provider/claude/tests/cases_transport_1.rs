//! Transport regressions.
use super::*;

#[test]
fn workspace_write_protects_the_outer_guard_directory_and_allows_only_its_socket() {
    let launcher = crate::launch::test_cache().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let config = SessionConfig {
        sandbox: Sandbox::WorkspaceWrite,
        ..SessionConfig::for_test(
            Uuid::new_v4().to_string(),
            dir.path().canonicalize().unwrap(),
        )
    };
    let mut command = Command::new("claude");
    let guard = config.configure(&mut command, &launcher).unwrap();
    let args: Vec<_> = command.get_args().collect();
    let settings: Value = serde_json::from_str(
        args.windows(2)
            .find(|pair| pair[0] == "--settings")
            .unwrap()[1]
            .to_str()
            .unwrap(),
    )
    .unwrap();
    let deny = settings["sandbox"]["filesystem"]["denyWrite"]
        .as_array()
        .unwrap();
    assert!(deny.contains(&json!(guard.protected_directory())));
    assert_ne!(guard.directory(), guard.protected_directory());
    assert_eq!(
        settings["sandbox"]["network"]["allowUnixSockets"],
        json!([guard.directory().join("socket")])
    );
}

#[test]
#[cfg(feature = "comb")]
fn abandoned_comb_callbacks_do_not_accumulate_cancelled_tombstones() {
    let mut f = Fixture::new();
    f.turn();
    let mut ids = Vec::new();
    for n in 0..300 {
        let native = if n < 2 {
            "retransmission".into()
        } else {
            format!("native-{n}")
        };
        f.codec.receive(json!({"type":"control_request","request_id":native,"request":{
            "subtype":"mcp_message","server_name":"hive","message":{"jsonrpc":"2.0","id":"same-inner-id","method":"tools/call","params":{"name":"comb_get","arguments":{"slug":"concepts/test"}}}
        }})).unwrap();
        let frame = f.codec.pop().unwrap();
        ids.push(frame["params"]["callId"].clone());
        let replies = f.codec.receive(json!({"type":"control_request","request_id":"cancel","request":{
            "subtype":"mcp_message","server_name":"hive","message":{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"same-inner-id"}}
        }})).unwrap();
        for reply in replies {
            f.codec.receive(reply).unwrap();
        }
        f.codec.forget_tool(&frame["id"]).unwrap();
    }
    assert_eq!(ids[0], ids[1]);
    assert_ne!(ids[1], ids[2]);
}

#[test]
#[cfg(feature = "comb")]
fn interrupt_retires_comb_callbacks_not_yet_delivered_to_the_worker() {
    let mut f = Fixture::new();
    let turn = f.turn();
    // Decode two callbacks before the worker handles even the first frame.
    for n in 0..2 {
        f.codec.receive(json!({"type":"control_request","request_id":format!("comb-{n}"),"request":{
            "subtype":"mcp_message","server_name":"hive","message":{"jsonrpc":"2.0","id":n,"method":"tools/call","params":{"name":"comb_get","arguments":{"slug":"concepts/test"}}}
        }})).unwrap();
    }
    let admitted = f.codec.pop().unwrap();
    f.codec.forget_comb_turn(&turn);
    f.codec.forget_tool(&admitted["id"]).unwrap();
    let interrupt = f
        .codec
        .send(outgoing::request(999, outgoing::interrupt(&f.id, &turn)))
        .unwrap();
    f.codec.receive(json!({"type":"control_response","response":{"subtype":"success","request_id":interrupt[0]["request_id"],"response":{"still_queued":[],"cancelled":[]}}})).unwrap();
    f.codec.receive(json!({"type":"result","session_id":f.id,"subtype":"success","is_error":false,"terminal_reason":"aborted_tools","user_message_uuid":turn})).unwrap();
    let queued = f.codec.pop().unwrap();
    assert_eq!(queued["method"], "item/tool/call");
    f.codec.forget_tool(&queued["id"]).unwrap(); // No unknown-ID reply.
    assert_eq!(f.codec.pop().unwrap()["id"], 999);
    assert_eq!(
        f.codec.pop().unwrap()["params"]["turn"]["status"],
        "interrupted"
    );
    assert_ne!(f.turn(), turn);
}
