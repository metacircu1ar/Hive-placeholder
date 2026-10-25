use super::{terminal_drain::LIMIT, test_support::RpcHarness};
use crate::runtime::provider::protocol::outgoing;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

fn finish(h: &mut RpcHarness) {
    h.ingest(&json!({"method":"turn/completed","params":{
        "threadId":"thread","turn":{"id":"turn","status":"interrupted"}}}))
        .unwrap();
}
fn item(thread: &str, turn: &str) -> Value {
    json!({"method":"item/started","params":{"threadId":thread,"turnId":turn,
        "item":{"id":"late","type":"commandExecution"}}})
}
fn armed() -> RpcHarness {
    let launcher = crate::launch::test_cache().unwrap();
    let mut h = RpcHarness::active(launcher.clone());
    h.rpc.arm_terminal_drain("thread", "turn").unwrap();
    finish(&mut h);
    h
}

#[test]
fn terminal_drain_is_armed_before_worker_completion_and_compares_activity_on_close() {
    let mut h = armed();
    // Event arrives before the worker asks for its first snapshot.
    h.ingest(&item("thread", "turn")).unwrap();
    let first = h.rpc.terminal_drain("thread", "turn", None).unwrap();
    assert_eq!(first.revision, 1);
    h.ingest(&item("thread", "turn")).unwrap();
    assert_eq!(
        h.rpc
            .terminal_drain("thread", "turn", Some(first.revision))
            .unwrap()
            .revision,
        2
    );
    assert!(h.begin(outgoing::turn_input("thread", "")).is_err());
    assert!(h.rpc.retire_thread("thread").is_err());
    assert!(
        h.rpc
            .terminal_drain("thread", "turn", Some(2))
            .unwrap()
            .closed_at
            .is_some()
    );
    assert!(h.rpc.terminal_drain("thread", "turn", Some(2)).is_err());
    assert!(h.rpc.terminal_drain("thread", "turn", None).is_err());
    assert!(h.rpc.arm_terminal_drain("thread", "turn").is_err());
    h.begin(outgoing::turn_input("thread", "")).unwrap();
    h.ingest(&item("thread", "turn")).unwrap();
    assert_eq!(
        h.rpc.thread_failure("thread").code(),
        "late_activity_after_drain"
    );
    assert!(h.rpc.thread_uncertain("thread"));
}

#[test]
fn terminal_drain_never_extends_its_deadline_or_exempts_expired_activity() {
    let mut h = armed();
    let first = h.rpc.terminal_drain("thread", "turn", None).unwrap();
    h.rpc.arm_terminal_drain("thread", "turn").unwrap();
    assert_eq!(
        h.rpc
            .terminal_drain("thread", "turn", None)
            .unwrap()
            .deadline,
        first.deadline
    );
    assert!(first.deadline <= Instant::now() + LIMIT);
    h.rpc
        .threads
        .get_mut("thread")
        .unwrap()
        .drain
        .as_mut()
        .unwrap()
        .deadline = Some(
        Instant::now()
            .checked_sub(Duration::from_millis(1))
            .unwrap(),
    );
    crate::test_support::assert_code!(
        h.rpc.terminal_drain("thread", "turn", None),
        "provider_timeout"
    );
    h.ingest(&item("thread", "turn")).unwrap();
    assert!(h.rpc.thread_uncertain("thread"));
}

#[test]
fn terminal_drain_rejects_unknown_turns_and_request_bearing_items() {
    for (thread, turn, request) in [
        ("thread", "wrong", false),
        ("unknown", "turn", false),
        ("thread", "turn", true),
    ] {
        let mut h = armed();
        let mut value = item(thread, turn);
        if request {
            value["id"] = json!("request");
        }
        h.ingest(&value).unwrap();
        assert!(h.rpc.thread_uncertain("thread") || h.rpc.uncertain());
    }
}

#[test]
fn terminal_drain_requires_local_interrupt_for_late_start_exemption() {
    let launcher = crate::launch::test_cache().unwrap();
    let mut h = RpcHarness::active(launcher.clone());
    finish(&mut h);
    h.rpc.terminal_drain("thread", "turn", None).unwrap();
    h.ingest(&item("thread", "turn")).unwrap();
    assert!(h.rpc.thread_uncertain("thread"));
    let mut h = RpcHarness::active(launcher.clone());
    finish(&mut h);
    // Natural completion already observed when the exact interrupt is issued.
    let before = h.rpc.terminal_drain("thread", "turn", None).unwrap();
    h.rpc.arm_terminal_drain("thread", "turn").unwrap();
    h.ingest(&item("thread", "turn")).unwrap();
    assert!(!h.rpc.thread_uncertain("thread"));
    assert_eq!(
        h.rpc
            .terminal_drain("thread", "turn", None)
            .unwrap()
            .deadline,
        before.deadline
    );
}

#[test]
fn terminal_drain_keeps_strict_post_window_and_shutdown_guards() {
    let mut h = armed();
    h.rpc.terminal_drain("thread", "turn", Some(0)).unwrap();
    h.ingest(&item("thread", "turn")).unwrap();
    assert!(h.rpc.thread_uncertain("thread"));
    assert_eq!(
        h.rpc.thread_failure("thread").code(),
        "late_activity_after_drain"
    );
    let mut h = armed();
    // Even reaping the provider cannot promote an unfinished observation into
    // recovery evidence. Raw shutdown clears the exemption and retains failure.
    h.rpc.pipes.child.kill().unwrap();
    let report = h.rpc.close();
    assert!(!report.confirmed && !report.quiescent);
    assert!(h.rpc.threads.values().all(|state| state.drain.is_none()));
}
