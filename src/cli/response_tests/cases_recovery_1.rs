//! Recovery regressions.
use super::*;

#[test]
fn gc_cli_retries_lost_reply_and_cleanup_with_same_identity_and_counts_once() {
    let next = Some(json!({"ended_at":1,"id":"a"}));
    let stub = Stub::replies(vec![
        None,
        Some(gc_page(10, true, next.clone())),
        Some(gc_page(10, false, next)),
        Some(gc_page(10, false, None)),
    ]);
    let result = super::super::gc::run_before(stub.root(), 10, false);
    assert!(result.success);
    assert_eq!(result.value["acknowledged_pages"], 2);
    assert_eq!(result.value["stats"]["deleted_agents"], 2);
    let requests = stub.finish();
    assert_eq!(requests[0], requests[1]);
    assert_eq!(requests[1], requests[2]);
    assert_ne!(requests[2]["id"], requests[3]["id"]);
    assert_eq!(requests[3]["args"]["after"], json!({"ended_at":1,"id":"a"}));
}

#[test]
fn gc_cli_accepts_the_control_sockets_request_receipt() {
    let mut page = gc_page(10, false, None);
    page["request_id"] = json!("receipt");
    let stub = Stub::new(vec![page]);
    let result = super::super::gc::run_before(stub.root(), 10, false);
    assert!(result.success, "{}", result.value);
    assert_eq!(result.value["stats"]["deleted_agents"], 1);
    assert_eq!(stub.finish().len(), 1);
}
