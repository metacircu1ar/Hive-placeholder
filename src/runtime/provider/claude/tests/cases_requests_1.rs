//! Requests regressions.
use super::*;

#[test]
fn mcp_error_and_cancellation_settle_distinct_request_ids() {
    let mut f = Fixture::new();
    f.turn();
    let frame = json!({"type":"control_request","request_id":"outer","request":{"subtype":"mcp_message","server_name":"hive","message":{"jsonrpc":"2.0","id":"inner","method":"tools/call","params":{"name":"hive_agents","arguments":{}}}}});
    f.codec.receive(frame.clone()).unwrap();
    let id = f.codec.pop().unwrap()["id"].clone();
    let error = f
        .codec
        .send(outgoing::unsupported(&id, "Test tool failure."))
        .unwrap();
    assert_eq!(error[0]["response"]["request_id"], "outer");
    assert_eq!(
        error[0]["response"]["response"]["mcp_response"]["id"],
        "inner"
    );
    assert!(error[0]["response"]["response"]["mcp_response"]["error"].is_object());
    f.codec.receive(frame).unwrap();
    let id = f.codec.pop().unwrap()["id"].clone();
    f.codec
        .receive(json!({"type":"control_cancel_request","request_id":"outer"}))
        .unwrap();
    assert!(
        f.codec
            .send(outgoing::unsupported(&id, "Test tool failure."))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn tools_list_keeps_the_public_schema_and_request_identity() {
    let mut fixture = Fixture::new();
    let mut expected = crate::runtime::api::tools().clone();
    for tool in expected.as_array_mut().unwrap() {
        tool.as_object_mut().unwrap().remove("type");
    }
    for id in [json!("opaque"), json!(17), json!(null)] {
        let replies = fixture
            .codec
            .receive(json!({
                "type":"control_request", "request_id":"tools", "request":{
                    "subtype":"mcp_message", "server_name":"hive", "message":{
                        "jsonrpc":"2.0", "id":id, "method":"tools/list", "params":{}
                    }
                }
            }))
            .unwrap();
        let response = &replies[0]["response"]["response"]["mcp_response"];
        assert_eq!(response["id"], id);
        assert_eq!(response["result"]["tools"], expected);
        assert_eq!(replies[0]["response"]["request_id"], "tools");
    }
}
