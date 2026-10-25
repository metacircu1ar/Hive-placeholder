use super::test_support::RpcHarness;
use crate::runtime::provider::protocol::outgoing;
use serde_json::json;

#[test]
fn steering_ack_is_bound_to_expected_turn_even_after_completion() {
    let launcher = crate::launch::test_cache().unwrap();
    for ended in [false, true] {
        let mut h = RpcHarness::active(launcher.clone());
        let id = h
            .begin(outgoing::steer("thread", "turn", "guidance"))
            .unwrap();
        if ended {
            h.ingest(&json!({"method":"turn/completed","params":{"threadId":"thread","turn":{"id":"turn","status":"completed"}}})).unwrap();
        }
        h.response(&json!({"id":id,"result":{"turnId":"turn"}}))
            .unwrap();
        assert_eq!(h.state("thread"), if ended { "idle" } else { "active" });
        assert!(!h.rpc.thread_uncertain("thread"));
    }
    for result in [
        json!({}),
        json!({"turnId":null}),
        json!({"turnId":"other"}),
        json!({"turn":{"id":"turn"}}),
    ] {
        let mut h = RpcHarness::active(launcher.clone());
        let id = h
            .begin(outgoing::steer("thread", "turn", "guidance"))
            .unwrap();
        crate::test_support::assert_code!(
            h.response(&json!({"id":id,"result":result})),
            "provider_protocol_error"
        );
        assert!(h.rpc.thread_uncertain("thread"));
    }
}

#[test]
fn steering_error_classification_never_confuses_validation_with_uncertain_delivery() {
    let launcher = crate::launch::test_cache().unwrap();
    for (code, message, expected) in [
        (-32600, "no active turn to steer", "no_active_turn"),
        (
            -32600,
            "expected active turn id `turn` but found `other`",
            "stale_turn",
        ),
        (
            -32600,
            "expected active turn id `wrong` but found `other`",
            "provider_request_failed",
        ),
        (
            -32600,
            "unrelated invalid request",
            "provider_request_failed",
        ),
        (-32602, "invalid parameters", "provider_request_failed"),
        (-32601, "unknown method", "unsupported_operation"),
        (-32603, "no active turn to steer", "steering_uncertain"),
        (-1, "unknown failure", "steering_uncertain"),
    ] {
        let mut h = RpcHarness::active(launcher.clone());
        let id = h
            .begin(outgoing::steer("thread", "turn", "guidance"))
            .unwrap();
        assert_eq!(
            h.response(&json!({"id":id,"error":{"code":code,"message":message}}))
                .unwrap_err()
                .code(),
            expected
        );
        assert_eq!(h.state("thread"), "active");
        assert!(!h.rpc.thread_uncertain("thread"));
    }
}
