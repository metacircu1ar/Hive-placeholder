//! Literal synthetic native controls pin IDs, payload bytes, and exact envelope fields.
use super::*;
use crate::runtime::api;

fn mcp(method: &str, id: &Value, params: Value) -> Value {
    json!({"type":"control_request","request_id":"outer","request":{
        "subtype":"mcp_message","server_name":"hive","message":{
        "jsonrpc":"2.0","id":id,"method":method,"params":params}}})
}
fn exact_control(actual: &[Value], inner: Value) {
    let expected = vec![json!({"type":"control_response","response":{
        "subtype":"success","request_id":"outer","response":{"mcp_response":inner}}})];
    assert_eq!(actual, expected);
    assert_eq!(
        serde_json::to_vec(actual).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
}
#[test]
fn native_mcp_metadata_replies_keep_opaque_ids_and_exact_fields() {
    for id in [
        Value::Null,
        json!(17),
        json!("opaque"),
        json!([]),
        json!({"raw":[null]}),
        json!(true),
        json!(0.25),
    ] {
        for method in [
            "initialize",
            "ping",
            "notifications/initialized",
            "future/method",
            "tools/list",
        ] {
            let mut f = Fixture::new();
            f.turn();
            let actual = f
                .codec
                .receive(mcp(method, &id, json!({"protocolVersion":"2024-11-05"})))
                .unwrap();
            let result = match method {
                "initialize" => json!({"protocolVersion":"2024-11-05","capabilities":{"tools":{}},
                    "serverInfo":{"name":"hive","version":env!("CARGO_PKG_VERSION")}}),
                "tools/list" => {
                    json!({"tools":api::tools().as_array().unwrap().iter().map(|t| json!({
                    "name":t["name"],"description":t["description"],"inputSchema":t["inputSchema"]})).collect::<Vec<_>>()})
                }
                _ => json!({}),
            };
            let inner = if method == "future/method" {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unsupported MCP method."}})
            } else {
                json!({"jsonrpc":"2.0","id":id,"result":result})
            };
            exact_control(&actual, inner);
        }
    }
}
#[test]
fn native_mcp_tool_replies_keep_opaque_ids_and_content_bytes() {
    for id in [
        json!("opaque"),
        json!({"raw":[null,true]}),
        json!([]),
        json!(false),
    ] {
        for success in [false, true] {
            let mut f = Fixture::new();
            f.turn();
            let args = json!({"external":[null,{"exact":"value"}]});
            assert!(
                f.codec
                    .receive(mcp(
                        "tools/call",
                        &id,
                        json!({"name":"hive_agents","arguments":args})
                    ))
                    .unwrap()
                    .is_empty()
            );
            let request = f.codec.pop().unwrap();
            assert_eq!(request["params"]["arguments"], args);
            let result = json!({"raw":[null,"text"]});
            let actual = f
                .codec
                .send(outgoing::tool_reply(&request["id"], &result, success))
                .unwrap();
            exact_control(
                &actual,
                json!({"jsonrpc":"2.0","id":id,"result":{
                "content":[{"type":"text","text":result.to_string()}],"isError":!success}}),
            );
            assert!(f.codec.pop().is_none());
        }
    }
}
#[test]
fn unsupported_native_control_has_exact_error_envelope() {
    let mut f = Fixture::new();
    f.turn();
    let actual = f
        .codec
        .receive(json!({"type":"control_request","request_id":"outer",
        "request":{"subtype":"future"}}))
        .unwrap();
    let expected = vec![json!({"type":"control_response","response":{
        "subtype":"error","request_id":"outer","error":"Unsupported control request."}})];
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
}

#[test]
fn native_control_request_shapes_match_literal_bytes() {
    use wire::control::{NativeRequest, OutboundControl};
    for (request, expected) in [
        (
            NativeRequest::initialize(),
            json!({"subtype":"initialize","hooks":{
            "PreToolUse":[{"hookCallbackIds":["hive-guard"]}],
            "PostToolUse":[{"hookCallbackIds":["hive-finished"]}],
            "PostToolUseFailure":[{"hookCallbackIds":["hive-finished"]}]}}),
        ),
        (
            NativeRequest::discovery(),
            json!({"subtype":"initialize","hooks":{}}),
        ),
        (
            NativeRequest::GetSettings,
            json!({"subtype":"get_settings"}),
        ),
        (
            NativeRequest::Interrupt {
                cancel_queued: true,
            },
            json!({"subtype":"interrupt","cancel_queued":true}),
        ),
    ] {
        let actual = api::value(OutboundControl {
            kind: "control_request",
            request_id: "id",
            request,
        });
        let expected = json!({"type":"control_request","request_id":"id","request":expected});
        assert_eq!(
            serde_json::to_vec(&actual).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
    }
}
#[test]
fn catalog_keeps_alias_order_and_first_descriptor_for_colliding_names() {
    let init: Initialization = serde_json::from_value(json!({
        "account":{"apiProvider":"firstParty"},"models":[
        {"value":"alias","resolvedModel":"resolved","displayName":"First","description":"one","supportedEffortLevels":["low","high"]},
        {"value":"resolved","resolvedModel":"other","displayName":"Second","description":"two"}]})).unwrap();
    let actual = catalog(&init).unwrap();
    let expected = json!({"data":[
        {"id":"alias","model":"alias","resolvedModel":"resolved","displayName":"First","description":"one","supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"high"}]},
        {"id":"resolved","model":"resolved","resolvedModel":"resolved","displayName":"First","description":"one","supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"high"}]},
        {"id":"other","model":"other","resolvedModel":"other","displayName":"Second","description":"two","supportedReasoningEfforts":[{"reasoningEffort":"none"}]}
    ],"nextCursor":null});
    assert_eq!(actual, expected);
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
}
