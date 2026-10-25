//! Provider frame preservation, response correlation and lifecycle requirements.
use super::test_support::RpcHarness;
use crate::runtime::protocol_corpus_support::{ExpectedRoute, frames};
use crate::runtime::provider::protocol::ClientMethod;
use crate::runtime::provider::protocol::outgoing;
use serde_json::{Value, json};

#[test]
fn protocol_corpus_fixture_frames_keep_raw_values_and_lifecycle_decisions() {
    let launcher = crate::launch::test_cache().unwrap();
    for case in frames() {
        let mut harness = RpcHarness::active(launcher.clone());
        let raw = harness.ingest(&case.frame).unwrap();
        assert_eq!(raw, case.frame, "{}", case.name);
        assert_eq!(raw.to_string(), case.frame.to_string(), "{}", case.name);
        assert_eq!(
            harness.rpc.uncertain(),
            case.route == ExpectedRoute::GlobalFailure,
            "{}",
            case.name
        );
        assert_eq!(
            harness.rpc.thread_uncertain("thread"),
            case.route == ExpectedRoute::OwnerFailure,
            "{}",
            case.name
        );
        assert_eq!(
            harness.rpc.active_turn("thread").unwrap(),
            case.active,
            "{}",
            case.name
        );
        if case.route == ExpectedRoute::Response {
            let id = harness
                .rpc
                .begin_request(crate::runtime::provider::protocol::outgoing::models(100))
                .unwrap();
            let mut response = case.frame.clone();
            response["id"] = json!(id);
            let result = harness.response(&response);
            if case.frame.get("error").is_some() {
                assert_eq!(
                    result.unwrap_err().code(),
                    "provider_request_failed",
                    "{}",
                    case.name
                );
            } else {
                assert_eq!(result.unwrap(), case.frame["result"], "{}", case.name);
            }
            assert!(!harness.outstanding(id));
        }
    }
}

#[test]
fn protocol_corpus_response_id_presence_and_numeric_types() {
    let launcher = crate::launch::test_cache().unwrap();
    let mut harness = RpcHarness::new(launcher.clone());
    let request = harness
        .rpc
        .begin_request(crate::runtime::provider::protocol::outgoing::models(100))
        .unwrap();
    for id in [
        None,
        Some(json!(null)),
        Some(json!("1")),
        Some(json!(1.0)),
        Some(json!(-1)),
        Some(json!(true)),
        Some(json!([])),
        Some(json!({})),
        Some(json!(request.checked_add(1).unwrap())),
    ] {
        let mut frame = json!({"result":null});
        if let Some(id) = id {
            frame["id"] = id;
        }
        crate::test_support::assert_code!(harness.response(&frame), "provider_protocol_error");
        assert!(harness.outstanding(request), "{frame}");
        assert!(!harness.rpc.uncertain(), "{frame}");
    }
    assert_eq!(
        harness
            .response(&json!({"id":request,"result":null}))
            .unwrap(),
        Value::Null
    );
    assert!(!harness.outstanding(request));
    crate::test_support::assert_code!(
        harness.response(&json!({"id":request,"result":null})),
        "provider_protocol_error"
    );
}

#[test]
fn protocol_corpus_result_and_error_presence_distinguishes_rejection_from_poison() {
    let launcher = crate::launch::test_cache().unwrap();
    let cases = [
        (json!({"result":null}), None),
        (json!({"result":{"opaque":[null,false,1]}}), None),
        (
            json!({"error":{"code":-1,"message":"private rejection","extra":true}}),
            Some("provider_request_failed"),
        ),
        (
            json!({"error":{"code":1.0,"message":"wrong code type"}}),
            Some("provider_protocol_error"),
        ),
        (
            json!({"error":{"code":-1,"message":false}}),
            Some("provider_protocol_error"),
        ),
        (json!({"error":null}), Some("provider_protocol_error")),
        (
            json!({"result":null,"error":null}),
            Some("provider_protocol_error"),
        ),
        (
            json!({"result":{},"error":{"code":-1,"message":"both"}}),
            Some("provider_protocol_error"),
        ),
        (json!({}), Some("provider_protocol_error")),
    ];
    for thread in [false, true] {
        for (body, expected) in &cases {
            let mut harness = RpcHarness::active(launcher.clone());
            let request = if thread {
                outgoing::terminal_list("thread")
            } else {
                outgoing::models(100)
            };
            let id = harness.rpc.begin_request(request).unwrap();
            let mut frame = body.clone();
            frame["id"] = json!(id);
            let result = harness.response(&frame);
            assert_eq!(
                result.as_ref().err().map(crate::error::Error::code),
                *expected,
                "{frame}"
            );
            if let Ok(value) = result {
                assert_eq!(value, body["result"]);
            }
            let malformed = *expected == Some("provider_protocol_error");
            assert_eq!(harness.rpc.uncertain(), malformed && !thread, "{frame}");
            assert_eq!(
                harness.rpc.thread_uncertain("thread"),
                malformed && thread,
                "{frame}"
            );
            assert!(!harness.outstanding(id));
        }
    }
}

#[test]
fn protocol_corpus_open_and_turn_acknowledgements_preserve_state_boundaries() {
    let launcher = crate::launch::test_cache().unwrap();
    for thread in [Value::Null, json!(5), json!(""), json!("x".repeat(1025))] {
        let mut harness = RpcHarness::new(launcher.clone());
        let id = harness
            .rpc
            .begin_request(crate::runtime::provider::protocol::outgoing::thread_start(
                None, None,
            ))
            .unwrap();
        crate::test_support::assert_code!(
            harness.response(&json!({"id":id,"result":{"thread":{"id":thread}}})),
            "provider_protocol_error"
        );
        assert!(harness.rpc.unresolved_lifecycle());
        assert!(!harness.rpc.uncertain());
    }
    let mut harness = RpcHarness::new(launcher.clone());
    let id = harness
        .rpc
        .begin_request(crate::runtime::provider::protocol::outgoing::thread_resume(
            None, "expected",
        ))
        .unwrap();
    crate::test_support::assert_code!(
        harness.response(&json!({"id":id,"result":{"thread":{"id":"other"}}})),
        "provider_protocol_error"
    );
    assert!(harness.rpc.uncertain());
    for turn in [
        Value::Null,
        json!(false),
        json!(""),
        json!("x".repeat(1025)),
    ] {
        let mut harness = RpcHarness::new(launcher.clone());
        harness.open();
        let id = harness
            .rpc
            .begin_request(crate::runtime::provider::protocol::outgoing::turn_input(
                "thread", "",
            ))
            .unwrap();
        assert_eq!(harness.state("thread"), "submitting");
        crate::test_support::assert_code!(
            harness.response(&json!({"id":id,"result":{"turn":{"id":turn}}})),
            "provider_protocol_error"
        );
        assert_eq!(harness.state("thread"), "submitting");
        assert!(harness.rpc.thread_uncertain("thread"));
        assert!(!harness.rpc.uncertain());
    }
}

#[test]
fn protocol_corpus_early_events_replay_raw_status_and_detect_conflicting_turns() {
    let launcher = crate::launch::test_cache().unwrap();
    for status in [
        json!("completed"),
        json!("failed"),
        json!("interrupted"),
        json!("future"),
        Value::Null,
        json!({"opaque":true}),
        json!(2),
    ] {
        let mut harness = RpcHarness::new(launcher.clone());
        harness.open();
        let id = harness
            .rpc
            .begin_request(crate::runtime::provider::protocol::outgoing::turn_input(
                "thread", "",
            ))
            .unwrap();
        let early = json!({"method":"turn/completed","params":{
            "threadId":"thread","turn":{"id":"turn","status":status,"extra":true}}});
        assert_eq!(harness.ingest(&early).unwrap(), early);
        harness
            .response(&json!({"id":id,"result":{"turn":{"id":"turn"}}}))
            .unwrap();
        let terminal = matches!(
            status.as_str(),
            Some("completed" | "failed" | "interrupted")
        );
        assert_eq!(
            harness.state("thread"),
            if terminal { "idle" } else { "active" }
        );
        assert_eq!(harness.rpc.thread_uncertain("thread"), !terminal);
    }
    for reject in [false, true] {
        let mut harness = RpcHarness::new(launcher.clone());
        harness.open();
        let id = harness
            .rpc
            .begin_request(crate::runtime::provider::protocol::outgoing::turn_input(
                "thread", "",
            ))
            .unwrap();
        harness
            .ingest(&json!({"method":"item/future/delta","params":{
            "threadId":"thread","turnId":"unexpected"}}))
            .unwrap();
        let response = if reject {
            json!({"id":id,"error":{"code":-1,"message":"turn rejected"}})
        } else {
            json!({"id":id,"result":{"turn":{"id":"turn"}}})
        };
        assert_eq!(harness.response(&response).is_err(), reject);
        assert!(harness.rpc.thread_uncertain("thread"));
        assert!(!harness.rpc.uncertain());
    }
}

#[test]
fn protocol_corpus_late_notifications_require_completed_identity_and_absent_request_id() {
    let launcher = crate::launch::test_cache().unwrap();
    for method in [
        "turn/completed",
        "item/completed",
        "item/future/delta",
        "item/started",
        "item/tool/call",
    ] {
        for has_id in [false, true] {
            let mut harness = RpcHarness::active(launcher.clone());
            harness
                .ingest(&json!({"method":"turn/completed","params":{
                "threadId":"thread","turn":{"id":"turn","status":"completed"}}}))
                .unwrap();
            harness.rpc.retire_thread("thread").unwrap();
            let mut frame = json!({"method":method,"params":{"threadId":"thread","turnId":"turn",
                "turn":{"id":"turn","status":"future-ignored-after-retirement"}}});
            if has_id {
                frame["id"] = Value::Null;
            }
            assert_eq!(harness.ingest(&frame).unwrap(), frame);
            assert_eq!(
                harness.rpc.thread_uncertain("thread"),
                has_id || method == "item/started"
            );
            assert!(!harness.rpc.uncertain());
            assert_eq!(harness.state("thread"), "absent");
        }
    }
}

#[test]
fn protocol_corpus_synchronous_matching_uses_raw_id_and_method_presence() {
    let launcher = crate::launch::test_cache().unwrap();
    for id in [json!(null), json!("1"), json!(1.0), json!(-1), json!(false)] {
        let mut harness = RpcHarness::new(launcher.clone());
        let result = harness.call_queued(&[json!({"id":id,"result":null})]);
        crate::test_support::assert_code!(result, "provider_protocol_error");
        assert!(harness.rpc.uncertain());
    }
    let mut harness = RpcHarness::new(launcher.clone());
    let response_id = harness.rpc.next_id.checked_add(1).unwrap();
    let notification = json!({"method":"unknown","params":{"opaque":true}});
    let result = harness
        .call_queued(&[
            json!({"id":null,"method":null,"params":null}),
            notification.clone(),
            json!({"id":response_id,"result":{"retained":null}}),
        ])
        .unwrap();
    assert_eq!(result, json!({"retained":null}));
    assert_eq!(harness.pending().unwrap(), notification);
    assert_eq!(
        harness.outgoing(2)[1],
        json!({"id":null,"error":{
        "code":-32601,"message":"Provider is cleaning up."}})
    );
    assert!(!harness.rpc.uncertain());
}

#[test]
fn protocol_corpus_invalid_json_poisoning_and_nonobject_frames() {
    let launcher = crate::launch::test_cache().unwrap();
    let mut harness = RpcHarness::new(launcher.clone());
    crate::test_support::assert_code!(
        harness.ingest_bytes(b"malformed protocol\n"),
        "provider_protocol_error"
    );
    assert!(harness.rpc.uncertain());
    for frame in [
        Value::Null,
        json!([]),
        json!(true),
        json!(7),
        json!("string"),
    ] {
        let mut harness = RpcHarness::new(launcher.clone());
        assert_eq!(harness.ingest(&frame).unwrap(), frame);
        assert!(!harness.rpc.uncertain());
    }
}

#[test]
fn protocol_corpus_lifecycle_ack_missing_and_wrong_containers_keep_failure_scope() {
    let launcher = crate::launch::test_cache().unwrap();
    for method in [ClientMethod::ThreadStart, ClientMethod::TurnStart] {
        for result in [
            json!(null),
            json!(false),
            json!(17),
            json!("wrong"),
            json!([]),
            json!({}),
            json!({"thread":null,"turn":null}),
            json!({"thread":[],"turn":[]}),
            json!({"thread":17,"turn":17}),
            json!({"thread":{},"turn":{}}),
        ] {
            let mut harness = RpcHarness::new(launcher.clone());
            if method == ClientMethod::TurnStart {
                harness.open();
            }
            let request = if method == ClientMethod::TurnStart {
                outgoing::turn_input("thread", "")
            } else {
                outgoing::thread_start(None, None)
            };
            let id = harness.rpc.begin_request(request).unwrap();
            assert_eq!(
                harness
                    .response(&json!({"id":id,"result":result}))
                    .unwrap_err()
                    .code(),
                "provider_protocol_error",
                "{method:?}: {result}"
            );
            assert!(!harness.rpc.uncertain(), "{method:?}: {result}");
            assert_eq!(
                harness.rpc.thread_uncertain("thread"),
                method == ClientMethod::TurnStart
            );
            assert!(harness.rpc.unresolved_lifecycle());
            assert!(!harness.outstanding(id));
        }
        let mut harness = RpcHarness::new(launcher.clone());
        if method == ClientMethod::TurnStart {
            harness.open();
        }
        let request = if method == ClientMethod::TurnStart {
            outgoing::turn_input("thread", "")
        } else {
            outgoing::thread_start(None, None)
        };
        let id = harness.rpc.begin_request(request).unwrap();
        crate::test_support::assert_code!(
            harness.response(&json!({"id":id})),
            "provider_protocol_error"
        );
        assert_eq!(harness.rpc.uncertain(), method == ClientMethod::ThreadStart);
        assert_eq!(
            harness.rpc.thread_uncertain("thread"),
            method == ClientMethod::TurnStart
        );
    }
}
