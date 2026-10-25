//! Lifecycle rejection, bounded early events and late request acknowledgements.
use super::test_support::RpcHarness;
use serde_json::json;

#[test]
fn early_turn_rejection_quarantines_thread() {
    let launcher = crate::launch::test_cache().unwrap();
    let mut harness = RpcHarness::new(launcher.clone());
    harness.open();
    let id = harness
        .rpc
        .begin_request(crate::runtime::provider::protocol::outgoing::turn_input(
            "thread", "",
        ))
        .unwrap();
    harness
        .ingest(&json!({"method":"turn/started","params":{
        "threadId":"thread","turn":{"id":"unexpected","status":"inProgress"}}}))
        .unwrap();
    crate::test_support::assert_code!(
        harness.response(&json!({"id":id,"error":{"code":-1,"message":"turn rejected"}})),
        "provider_request_failed"
    );
    assert!(harness.rpc.thread_uncertain("thread"));
    assert_eq!(harness.state("thread"), "idle");
}

#[test]
fn early_turn_buffer_is_bounded() {
    let launcher = crate::launch::test_cache().unwrap();
    let mut harness = RpcHarness::new(launcher.clone());
    harness.open();
    harness
        .rpc
        .begin_request(crate::runtime::provider::protocol::outgoing::turn_input(
            "thread", "",
        ))
        .unwrap();
    for index in 0..=super::EARLY_TURN_LIMIT {
        harness
            .ingest(&json!({"method":"turn/started","params":{
            "threadId":"thread","turn":{"id":"turn","status":"inProgress"}}}))
            .unwrap();
        assert_eq!(
            harness.rpc.thread_uncertain("thread"),
            index == super::EARLY_TURN_LIMIT
        );
    }
    assert_eq!(
        harness.rpc.threads["thread"].early.len(),
        super::EARLY_TURN_LIMIT
    );
}

#[test]
fn timed_out_model_list_accepts_late_ack() {
    let launcher = crate::launch::test_cache().unwrap();
    let mut harness = RpcHarness::new(launcher.clone());
    crate::test_support::assert_code!(harness.call_queued(&[]), "provider_timeout");
    let request = harness.outgoing(1)[0]["id"].as_u64().unwrap();
    assert!(harness.outstanding(request));
    assert!(!harness.rpc.uncertain());
    assert_eq!(
        harness
            .response(&json!({"id":request,"result":{"late":true}}))
            .unwrap(),
        json!({"late":true})
    );
    assert!(!harness.outstanding(request));
}
