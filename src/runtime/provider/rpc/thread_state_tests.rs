//! Retirement must forget cleanup membership while retaining native evidence.
use super::test_support::RpcHarness;
use serde_json::{Value, json};

fn completed() -> RpcHarness {
    let launcher = crate::launch::test_cache().unwrap();
    let mut harness = RpcHarness::active(launcher.clone());
    harness
        .ingest(&json!({"method":"turn/completed","params":{
        "threadId":"thread","turn":{"id":"turn","status":"completed"}}}))
        .unwrap();
    harness
}
fn passive() -> Value {
    json!({"method":"item/completed","params":{
        "threadId":"thread","turnId":"turn","item":{"id":"item","type":"commandExecution"}}})
}

#[test]
fn retirement_keeps_completion_evidence_and_excludes_the_thread_from_shutdown_cleanup() {
    let mut harness = completed();
    harness.rpc.retire_thread("thread").unwrap();
    assert_eq!(harness.state("thread"), "absent");
    assert!(harness.rpc.active_turn("thread").is_err());
    harness.ingest(&passive()).unwrap();
    assert!(!harness.rpc.uncertain());
    assert!(!harness.rpc.thread_uncertain("thread"));
    let report = harness.rpc.close();
    assert!(report.confirmed && report.quiescent && report.reaped);
    // The capture child cannot acknowledge cleanup RPCs: only initial thread/turn
    // requests were sent, so retained metadata was not treated as a live thread.
    assert_eq!(harness.outgoing(2).len(), 2);
}

#[test]
fn retired_terminal_drain_keeps_late_activity_attributed_to_its_thread() {
    let mut harness = completed();
    harness.rpc.terminal_drain("thread", "turn", None).unwrap();
    harness
        .rpc
        .terminal_drain("thread", "turn", Some(0))
        .unwrap();
    harness.rpc.retire_thread("thread").unwrap();
    harness.ingest(&passive()).unwrap();
    assert!(harness.rpc.thread_uncertain("thread"));
    assert!(!harness.rpc.uncertain());
    assert_eq!(
        harness.rpc.thread_failure("thread").code(),
        "late_activity_after_drain"
    );
    assert!(
        harness
            .rpc
            .begin_request(crate::runtime::provider::protocol::outgoing::turn_input(
                "thread", ""
            ))
            .is_err()
    );
}

#[test]
fn reopening_a_retired_thread_preserves_evidence_until_a_new_turn_is_submitted() {
    let mut harness = completed();
    harness.rpc.retire_thread("thread").unwrap();
    let resume = harness
        .rpc
        .begin_request(crate::runtime::provider::protocol::outgoing::thread_resume(
            None, "thread",
        ))
        .unwrap();
    harness
        .response(&json!({"id":resume,"result":{"thread":{"id":"thread"}}}))
        .unwrap();
    assert_eq!(harness.state("thread"), "idle");
    harness.ingest(&passive()).unwrap();
    assert!(!harness.rpc.thread_uncertain("thread"));
    harness
        .rpc
        .begin_request(crate::runtime::provider::protocol::outgoing::turn_input(
            "thread", "",
        ))
        .unwrap();
    assert!(harness.rpc.threads["thread"].completed.is_none());
    assert_eq!(harness.state("thread"), "submitting");
}
