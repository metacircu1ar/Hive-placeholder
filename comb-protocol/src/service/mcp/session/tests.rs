use super::*;
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
fn connection() -> Connection {
    Connection {
        format: hive_input::compatibility::Exact,
        socket: "/unused-comb-test.sock".into(),
        credential: crate::service::auth::Credential {
            comb_id: crate::values::CombId::generate(),
            id: crate::values::CredentialId::human(),
            secret: "unused".into(),
        },
    }
}
use serde_json::json;

#[test]
fn invalid_retrieval_tools_never_contact_a_listening_socket() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let socket = dir.path().join("sentinel.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut connection = connection();
    connection.socket = socket;
    let mut session = Session {
        connection,
        phase: SessionPhase::Ready,
    };
    runtime().block_on(async {
        for (name, arguments) in [
            ("comb_search", json!({})),
            ("comb_list", json!({"limit":0})),
            ("private-tool-name", json!({})),
        ] {
            let reply = request(
                &mut session,
                "tools/call",
                json!({"name":name,"arguments":arguments}),
            )
            .await;
            assert_eq!(reply["result"]["isError"], true);
            let content: Value =
                serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap())
                    .unwrap();
            assert!(content["error"]["field"].is_string());
            assert!(!content.to_string().contains("private-tool-name"));
        }
    });
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

async fn request(session: &mut Session, method: &str, params: Value) -> Value {
    let frame = json!({"jsonrpc":"2.0","id":"id","method":method,"params":params});
    value(
        session
            .handle(frame.to_string().as_bytes())
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap()
}
fn parameters() -> Value {
    json!({"protocolVersion":"any-string","clientInfo":{},"capabilities":{},"vendor":true})
}

#[test]
fn invalid_initialize_can_retry_then_ping_precedes_ready_notification() {
    let connection = connection();
    let mut session = Session {
        connection,
        phase: SessionPhase::Uninitialized,
    };
    runtime().block_on(async {
        assert_eq!(
            request(&mut session, "initialize", Value::Null).await["error"]["code"],
            -32602
        );
        let initialized = request(&mut session, "initialize", parameters()).await;
        assert_eq!(initialized["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(
            request(&mut session, "ping", Value::Null).await["result"],
            json!({})
        );
        assert_eq!(
            request(&mut session, "initialize", parameters()).await["error"]["code"],
            -32000
        );
        assert_eq!(
            request(&mut session, "tools/list", Value::Null).await["error"]["code"],
            -32000
        );
        for _ in 0..2 {
            assert!(
                session
                    .handle(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(
            request(&mut session, "initialize", parameters()).await["error"]["code"],
            -32601
        );
        assert!(
            session
                .handle(br#"{"jsonrpc":"2.0","method":"vendor/unknown"}"#)
                .await
                .unwrap()
                .is_none()
        );
    });
}

#[test]
fn tool_envelope_failures_stay_distinct_from_tool_argument_failures() {
    let connection = connection();
    let mut session = Session {
        connection,
        phase: SessionPhase::Ready,
    };
    runtime().block_on(async {
        assert_eq!(
            request(&mut session, "tools/call", json!({"arguments":{}})).await["error"]["code"],
            -32602
        );
        let missing = request(&mut session, "tools/call", json!({"name":"comb_get"})).await;
        let null = request(
            &mut session,
            "tools/call",
            json!({"name":"comb_get","arguments":null}),
        )
        .await;
        for reply in [&missing, &null] {
            assert!(reply.get("error").is_none());
            assert_eq!(reply["result"]["isError"], true);
            assert!(reply["result"].get("structuredContent").is_none());
        }
        let missing: Value =
            serde_json::from_str(missing["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        let null: Value =
            serde_json::from_str(null["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(missing["error"]["code"], "missing_argument");
        assert_eq!(missing["error"]["field"], "/arguments/slug");
        assert_eq!(null["error"]["code"], "invalid_value");
        assert_eq!(null["error"]["field"], "/arguments");
    });
}

#[test]
fn malformed_json_emits_one_error_then_transport_continues() {
    let connection = connection();
    let mut output = Vec::new();
    runtime()
        .block_on(run(
            connection,
            &b"{\n{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\"}\n"[..],
            &mut output,
        ))
        .unwrap();
    let replies = output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(replies.len(), 2);
    assert_eq!(
        replies[0],
        json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Invalid JSON."}})
    );
    assert_eq!(replies[1]["id"], 9);
    assert_eq!(replies[1]["error"]["code"], -32000);
}

#[test]
fn reviewer_capability_requires_an_object_with_true_reviewer_field() {
    assert!(
        reviewer_access(json!({"protocols":{"minimum":1,"maximum":1},"reviewer":true})).unwrap()
    );
    assert!(
        !reviewer_access(json!({"protocols":{"minimum":1,"maximum":1},"reviewer":false})).unwrap()
    );
    assert!(
        reviewer_access(json!({"protocols":{"minimum":1,"maximum":1},"reviewer":true,"extra":1}))
            .is_err()
    );
    for reply in [
        json!([true]),
        json!(true),
        Value::Null,
        json!({}),
        json!({"reviewer":null}),
        json!({"reviewer":"true"}),
        json!({"reviewer":1}),
    ] {
        assert_eq!(
            reviewer_access(reply).unwrap_err().code,
            crate::error::ErrorCode::InvalidFrame
        );
    }
}

#[test]
fn discovery_failure_does_not_reset_ready_session_or_echo_native_details() {
    let mut session = Session {
        connection: connection(),
        phase: SessionPhase::Ready,
    };
    runtime().block_on(async {
        let reply = request(&mut session, "tools/list", Value::Null).await;
        assert_eq!(reply["error"]["code"], -32000);
        assert!(
            reply["error"]["message"]
                .as_str()
                .unwrap()
                .contains("configured socket")
        );
        assert!(session.phase == SessionPhase::Ready);
        assert_eq!(
            request(&mut session, "ping", Value::Null).await["result"],
            json!({})
        );
    });
    let failure =
        discovery_failure(Error::forbidden().caused_by(std::io::Error::other("private-secret")));
    assert!(failure.message.contains("valid client credential"));
    assert!(!failure.message.contains("private-secret"));
}

#[test]
fn malformed_capabilities_fail_discovery_and_a_later_valid_reply_can_retry() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let socket = dir.path().join("capabilities.sock");
    runtime().block_on(async {
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let peer = tokio::spawn(async move {
            for data in [
                json!({"reviewer":"private-secret"}),
                json!({"protocols":{"minimum":1,"maximum":1},"reviewer":false}),
                json!({"protocols":{"minimum":1,"maximum":1},"reviewer":true}),
            ] {
                let (stream, _) = listener.accept().await.unwrap();
                let mut stream = tokio::io::BufReader::new(stream);
                let bytes = framing::read(&mut stream, framing::REQUEST_BYTES)
                    .await
                    .unwrap()
                    .unwrap();
                let op: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(op["operation"]["op"], "capabilities");
                framing::write(
                    stream.get_mut(),
                    &json!({"protocol":1,"status":"ok","result":data}),
                    framing::RESPONSE_BYTES,
                )
                .await
                .unwrap();
            }
        });
        let mut connection = connection();
        connection.socket = socket;
        let mut session = Session {
            connection,
            phase: SessionPhase::Ready,
        };
        let reply = request(&mut session, "tools/list", Value::Null).await;
        assert_eq!(reply["error"]["code"], -32000);
        assert!(
            reply["error"]["message"]
                .as_str()
                .unwrap()
                .contains("invalid capabilities")
        );
        assert!(!reply.to_string().contains("private-secret"));
        assert!(session.phase == SessionPhase::Ready);
        for reviewer in [false, true] {
            let reply = request(&mut session, "tools/list", Value::Null).await;
            let tools = reply["result"]["tools"].as_array().unwrap();
            assert_eq!(
                tools.iter().any(|tool| tool["name"] == "comb_promote"),
                reviewer
            );
        }
        peer.await.unwrap();
    });
}
