use super::*;
use crate::runtime::provider::{RpcHarness, protocol::IncomingFrame};
use serde_json::json;

fn queue(rpc: &mut Rpc, frames: &[Value]) {
    for raw in frames {
        let frame = IncomingFrame::parse(raw.clone());
        let bytes = frame.encoded_len();
        rpc.pending.push_back(frame, bytes);
    }
}
fn rejected(id: u64) -> Value {
    json!({"id":id,"error":{"code":-1,"message":"private native rejection"}})
}

#[test]
fn retries_catalog_rejections_without_reinitializing_or_changing_cursor() {
    let mut harness = RpcHarness::new(crate::launch::test_cache().unwrap());
    queue(
        &mut harness.rpc,
        &[
            rejected(1),
            rejected(2),
            json!({"id":3,"result":{"data":[],"nextCursor":null}}),
        ],
    );
    let mut delays = Vec::new();
    let reply = ModelLookup::default()
        .page_with(&mut harness.rpc, &json!("page-2"), |d| delays.push(d))
        .unwrap();
    assert!(reply.cursor().is_null());
    assert_eq!(delays, RETRY_DELAYS);
    let requests = harness.outgoing(3);
    assert_eq!(requests.len(), 3);
    for (index, request) in requests.iter().enumerate() {
        assert_eq!(request["id"], index + 1);
        assert_eq!(request["method"], "model/list");
        assert_eq!(request["params"]["cursor"], "page-2");
    }
}

#[test]
fn retry_budget_is_shared_across_pages_and_exhaustion_returns_final_error() {
    let mut harness = RpcHarness::new(crate::launch::test_cache().unwrap());
    queue(
        &mut harness.rpc,
        &[
            rejected(1),
            json!({"id":2,"result":{"data":[],"nextCursor":"second"}}),
            rejected(3),
            rejected(4),
        ],
    );
    let mut lookup = ModelLookup::default();
    let page = lookup
        .page_with(&mut harness.rpc, &Value::Null, |_| {})
        .unwrap();
    assert_eq!(page.cursor(), "second");
    let error = lookup
        .page_with(&mut harness.rpc, page.cursor(), |_| {})
        .err()
        .unwrap();
    assert_eq!(error.code(), "provider_request_failed");
    assert_eq!(lookup.retries, 2);
    assert_eq!(harness.outgoing(4).len(), 4);
}

#[test]
fn malformed_reply_is_not_retried_and_ordinary_calls_still_fail_once() {
    for frame in [
        json!({"id":1}),
        json!({"id":1,"result":{},"error":{"code":-1,"message":"both"}}),
    ] {
        let mut harness = RpcHarness::new(crate::launch::test_cache().unwrap());
        queue(&mut harness.rpc, &[frame]);
        let error = ModelLookup::default()
            .page_with(&mut harness.rpc, &Value::Null, |_| panic!("fatal retry"))
            .err()
            .unwrap();
        assert_eq!(error.code(), "provider_protocol_error");
        assert_eq!(harness.outgoing(1).len(), 1);
    }
    let mut harness = RpcHarness::new(crate::launch::test_cache().unwrap());
    queue(&mut harness.rpc, &[rejected(1)]);
    assert_eq!(
        harness
            .rpc
            .call(outgoing::initialize(false))
            .err()
            .unwrap()
            .code(),
        "provider_request_failed"
    );
    assert_eq!(harness.outgoing(1).len(), 1);
}

#[test]
fn slow_success_keeps_the_original_response_allowance() {
    let launcher = crate::launch::test_cache().unwrap();
    let mut command = std::process::Command::new("/bin/sh");
    command.args([
        "-c",
        r#"
        IFS= read -r first
        sleep 11
        printf '%s\n' '{"id":1,"result":{"data":[],"nextCursor":null}}'
        exec cat >/dev/null
    "#,
    ]);
    let mut rpc = Rpc::start(&launcher, command, vec![]).unwrap();
    let mut lookup = ModelLookup::default();
    // A ten-second attempt slice would regress this healthy serial provider.
    let page = lookup.page(&mut rpc, &Value::Null).unwrap();
    assert!(page.cursor().is_null());
    assert_eq!(lookup.retries, 0);
    assert_eq!(rpc.next_id, 1);
    assert!(rpc.close().confirmed);
}

#[test]
fn timeout_is_not_retried_and_keeps_its_late_response_identity() {
    let mut harness = RpcHarness::new(crate::launch::test_cache().unwrap());
    let mut lookup = ModelLookup {
        retries: 0,
        deadline: Instant::now() + Duration::from_millis(20),
    };
    let error = lookup.page(&mut harness.rpc, &Value::Null).err().unwrap();
    assert_eq!(error.code(), "provider_timeout");
    assert_eq!(lookup.retries, 0);
    assert!(harness.outstanding(1));
    assert_eq!(harness.outgoing(1).len(), 1);
    harness
        .response(&json!({"id":1,"result":{"data":[],"nextCursor":null}}))
        .unwrap();
    assert!(!harness.outstanding(1));
    assert!(!harness.rpc.uncertain());
}

#[test]
fn expired_lookup_budget_never_sends_another_page() {
    let mut harness = RpcHarness::new(crate::launch::test_cache().unwrap());
    let mut lookup = ModelLookup {
        retries: 0,
        deadline: Instant::now(),
    };
    let error = lookup
        .page(&mut harness.rpc, &json!("later-page"))
        .err()
        .unwrap();
    assert_eq!(error.code(), "provider_timeout");
    assert_eq!(harness.rpc.next_id, 0);
}

#[test]
fn malformed_late_reply_cannot_publish_a_successful_retry() {
    let mut harness = RpcHarness::new(crate::launch::test_cache().unwrap());
    // Retain a timed-out metadata request, then encounter its malformed reply
    // while awaiting the retry's valid response.
    harness
        .rpc
        .begin_request(outgoing::model_page(MODEL_PAGE_LIMIT, &Value::Null))
        .unwrap();
    queue(
        &mut harness.rpc,
        &[
            json!({"id":1,"result":{},"error":{"code":-1,"message":"both"}}),
            json!({"id":2,"result":{"data":[],"nextCursor":null}}),
        ],
    );
    let error = ModelLookup::default()
        .page(&mut harness.rpc, &Value::Null)
        .err()
        .unwrap();
    assert_eq!(error.code(), "provider_protocol_error");
    assert!(harness.rpc.uncertain());
    assert_eq!(harness.outgoing(2).len(), 2);
}
