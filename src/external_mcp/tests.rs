use super::{wire::*, *};
use serde_json::json;

fn frame(raw: Value) -> Frame {
    match wire::parse(&serde_json::to_vec(&raw).unwrap()) {
        Ok(frame) => frame,
        Err(error) => panic!("{}", serde_json::to_string(&error).unwrap()),
    }
}
fn wire_value(message: Message) -> Value {
    serde_json::from_slice(&serde_json::to_vec(&message).unwrap()).unwrap()
}
fn assert_wire(message: Message, expected: Value) {
    assert_eq!(
        serde_json::to_vec(&message).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
}

#[test]
fn external_tool_catalog_preserves_frozen_wire_bytes() {
    if std::env::var_os("HIVE_UPDATE_TOOL_SCHEMAS").is_some() {
        std::fs::write(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/external_mcp/tool_schemas.json"
            ),
            format!("{}\n", serde_json::to_string(&tools::catalog()).unwrap()),
        )
        .unwrap();
        return;
    }
    assert_eq!(
        serde_json::to_string(&tools::catalog()).unwrap(),
        include_str!("tool_schemas.json").trim_end()
    );
}

#[test]
fn typed_messages_keep_exact_sorted_wire_bytes_and_opaque_payloads() {
    let id = RequestId::Text("quoted\"雪".into());
    assert_wire(
        Message::reply(id, ResultBody::empty()),
        json!({"jsonrpc":"2.0","id":"quoted\"雪","result":{}}),
    );
    assert_wire(
        Message::error(None, -32600, "Invalid JSON-RPC envelope."),
        json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Invalid JSON-RPC envelope."}}),
    );
    assert_wire(
        Message::progress(
            ProgressToken::Number(serde_json::Number::from_f64(1.5).unwrap()),
            2,
            "raw \"\n雪".into(),
        ),
        json!({"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":1.5,"progress":2,"message":"raw \"\n雪"}}),
    );
    let raw =
        json!({"z":[null,true,{"foreign":"雪\n\"","n":18_446_744_073_709_551_615_u64}],"a":-1.5});
    assert_wire(
        Message::reply(
            RequestId::Number(1.into()),
            ResultBody::tool(raw.clone(), true),
        ),
        json!({"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":raw.to_string()}],"structuredContent":raw,"isError":true}}),
    );
    assert_wire(
        Message::reply(
            RequestId::Number(2.into()),
            ResultBody::catalog(tools::catalog()),
        ),
        json!({"jsonrpc":"2.0","id":2,"result":{"tools":tools::catalog()}}),
    );
    assert_wire(
        Message::reply(
            RequestId::Number(3.into()),
            ResultBody::initialize(PROTOCOL),
        ),
        json!({"jsonrpc":"2.0","id":3,"result":{"protocolVersion":PROTOCOL,"capabilities":{"tools":{}},"serverInfo":{"name":"hive-external","version":env!("CARGO_PKG_VERSION"),"icons":[{"src":include_str!("../../icons/generated/hive-64.data-uri"),"mimeType":"image/png","sizes":["64x64"]}]},"instructions":"External callers only. Long calls are resumed by message ID; timeouts never cancel delivery. Reuse request_id for retries."}}),
    );
}

#[test]
fn envelope_rejection_retains_null_priority_and_duplicate_key_handling() {
    for (bytes, message) in [
        (b"{".as_slice(), "Invalid JSON."),
        (b"[]", "JSON-RPC frames must be objects."),
        (b"{\"id\":null}", "Request ID must not be null."),
        (
            b"{\"jsonrpc\":0,\"method\":null,\"id\":null}",
            "Request ID must not be null.",
        ),
        (
            b"{\"jsonrpc\":\"2.0\",\"method\":\"ping\",\"id\":1.5}",
            "Invalid JSON-RPC envelope.",
        ),
        (
            b"{\"jsonrpc\":\"2.0\",\"method\":\"ping\",\"id\":{}}",
            "Invalid JSON-RPC envelope.",
        ),
    ] {
        let Err(error) = wire::parse(bytes) else {
            panic!("accepted invalid envelope");
        };
        assert_eq!(wire_value(error)["error"]["message"], message);
    }
    let parsed =
        wire::parse(br#"{"jsonrpc":"2.0","method":"ping","id":null,"id":18446744073709551615}"#)
            .ok()
            .unwrap();
    assert_eq!(parsed.id.unwrap().key(), "18446744073709551615");
    assert!(frame(json!({"jsonrpc":"2.0","method":"ping"})).id.is_none());
}

#[test]
fn initialization_projections_require_objects_and_keep_opaque_extensions() {
    for bad in [
        json!([]),
        json!(["version", {}, {}]),
        json!({"protocolVersion":"","clientInfo":[],"capabilities":{}}),
        json!({"protocolVersion":"","clientInfo":{},"capabilities":null}),
    ] {
        assert!(!wire::valid_initialize(bad));
    }
    assert!(wire::valid_initialize(
        json!({"protocolVersion":"future","clientInfo":{"unknown":[null,17]},"capabilities":{"future":{}},"extra":true})
    ));
}

#[test]
fn tool_metadata_distinguishes_absence_null_invalid_and_opaque_fields() {
    let params = json!({"name":"hive_wait_message","arguments":{"message_id":"m"}});
    assert!(tools::parse(params.clone()).is_ok());
    for meta in [
        Value::Null,
        json!([]),
        json!(17),
        json!({"extra":{}}),
        json!({"progressToken":""}),
        json!({"progressToken":1.5}),
    ] {
        let mut candidate = params.clone();
        candidate["_meta"] = meta;
        assert!(tools::parse(candidate).is_ok());
    }
    for token in [Value::Null, json!(false), json!([]), json!({})] {
        let mut candidate = params.clone();
        candidate["_meta"] = json!({"progressToken":token});
        assert!(tools::parse(candidate).is_err());
    }
    assert!(tools::parse(json!({"name":"hive_wait_message","arguments":["m",1]})).is_err());
    assert!(tools::parse(json!(["hive_wait_message",{"message_id":"m"}])).is_err());
}

#[tokio::test]
async fn notification_capacity_and_duplicate_id_precedence_is_unchanged() {
    let (updates, _) = mpsc::channel(64);
    let root = std::path::Path::new("/unused");
    let mut server = Server::default();
    assert!(
        server
            .handle(
                frame(json!({"jsonrpc":"2.0","method":"notifications/initialized"})),
                root,
                &updates
            )
            .is_none()
    );
    assert!(server.phase == McpPhase::New);
    let reply=server.handle(frame(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"any","clientInfo":{},"capabilities":{}}})),root,&updates).unwrap();
    assert_eq!(wire_value(reply)["result"]["protocolVersion"], PROTOCOL);
    server.notification(&frame(
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    ));
    for id in 0..MAX_CALLS {
        server.active.insert(
            id.to_string(),
            Active {
                generation: id.to_string(),
                task: tokio::spawn(std::future::pending()),
            },
        );
    }
    let duplicate = server
        .handle(
            frame(json!({"jsonrpc":"2.0","id":0,"method":"tools/call","params":null})),
            root,
            &updates,
        )
        .unwrap();
    assert_eq!(
        wire_value(duplicate)["error"]["message"],
        "Request ID is already active."
    );
    let capacity = server
        .handle(
            frame(json!({"jsonrpc":"2.0","id":100,"method":"tools/call","params":null})),
            root,
            &updates,
        )
        .unwrap();
    assert_eq!(
        wire_value(capacity)["error"]["message"],
        "Observer capacity reached; retry later."
    );
    server.notification(&frame(
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":0.0}}),
    ));
    assert_eq!(server.active.len(), MAX_CALLS);
    server.notification(&frame(
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":0}}),
    ));
    assert_eq!(server.active.len(), MAX_CALLS - 1);
}

#[test]
fn invalid_tool_calls_are_immediate_and_do_not_admit_observers() {
    let (updates, mut received) = mpsc::channel(64);
    let mut server = Server {
        phase: McpPhase::Ready,
        ..Server::default()
    };
    // No Tokio runtime: accidentally starting an observer would panic here.
    let root = std::path::Path::new("/unused");
    for (params, protocol, field) in [
        (json!({"name":"secret-canary"}), true, "/name"),
        (
            json!({"name":"hive_call","arguments":[]}),
            true,
            "/arguments",
        ),
        (json!({"name":"hive_call"}), false, "/arguments/target"),
        (
            json!({"name":"hive_call","arguments":{"target":"a","body":"b","request_id":"r","timeout":0}}),
            false,
            "/arguments/timeout",
        ),
        (
            json!({"name":"hive_wait_message","arguments":{"message_id":false}}),
            false,
            "/arguments/message_id",
        ),
        (
            json!({"name":"hive_output","arguments":{"id":"secret-canary"}}),
            false,
            "/arguments/id",
        ),
    ] {
        let message = server
            .handle(
                frame(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":params})),
                root,
                &updates,
            )
            .unwrap();
        let bytes = serde_json::to_vec(&message).unwrap();
        let value = wire_value(message);
        let error = if protocol {
            assert_eq!(value["error"]["code"], -32602);
            assert!(value.get("result").is_none());
            &value["error"]["data"]
        } else {
            assert!(value.get("error").is_none());
            let result = &value["result"];
            assert_eq!(result["isError"], true);
            assert_eq!(
                serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap())
                    .unwrap(),
                result["structuredContent"]
            );
            &result["structuredContent"]["error"]
        };
        assert_eq!(value["id"], 1);
        assert_eq!(error["field"], field);
        assert!(!value.to_string().contains("secret-canary"));
        assert!(error.get("received").is_none());
        assert_eq!(bytes, serde_json::to_vec(&value).unwrap());
        assert!(server.active.is_empty());
        assert!(received.try_recv().is_err());
    }
}
