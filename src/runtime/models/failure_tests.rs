use super::*;
use crate::runtime::test_support::{RuntimeFixture, failure_log};
use std::error::Error as _;

#[test]
fn standalone_catalog_retries_pages_without_duplicate_entries() {
    use crate::runtime::provider::{RpcHarness, protocol::IncomingFrame};
    use serde_json::json;
    let mut harness = RpcHarness::new(crate::launch::test_cache().unwrap());
    for raw in [
        json!({"id":1,"result":{"data":[{"model":"first","opaque":true}],"nextCursor":"second"}}),
        json!({"id":2,"error":{"code":-1,"message":"fixture rejection"}}),
        json!({"id":3,"result":{"data":[{"model":"second"}],"nextCursor":null}}),
    ] {
        let frame = IncomingFrame::parse(raw);
        let bytes = frame.encoded_len();
        harness.rpc.pending.push_back(frame, bytes);
    }
    assert_eq!(
        model_pages(&mut harness.rpc).unwrap(),
        json!({"data":[{"model":"first","opaque":true},{"model":"second"}],"nextCursor":null})
    );
    let requests = harness.outgoing(3);
    assert_eq!(requests[1]["params"], requests[2]["params"]);
}

#[test]
fn discovery_and_receipt_failure_keep_native_cause() {
    let Some(log) = failure_log(
        module_path!(),
        "discovery_and_receipt_failure_keep_native_cause",
        || {
            let fixture = RuntimeFixture::new();
            let state = State::new(fixture.temp.path().join("state")).unwrap();
            let profile = crate::runtime::test_support::agent().profile;
            let binding = state.lookup("main", &profile).unwrap().unwrap();
            let binary = crate::runtime::test_support::failing_catalog(&fixture);
            let ModelDiscovery { result, confirmed } =
                models_owned(&fixture.launcher, &state, &binding, &binary, None);
            assert!(!confirmed);
            assert_eq!(
                result.as_ref().unwrap_err().code(),
                "provider_request_failed"
            );
            let recovery = recovery_error(result);
            assert_eq!(recovery.code(), "recovery_required");
            assert_eq!(
                recovery
                    .source()
                    .unwrap()
                    .downcast_ref::<Error>()
                    .unwrap()
                    .code(),
                "provider_request_failed"
            );
            fixture.finish();
        },
    ) else {
        return;
    };
    assert!(log.contains("models.cleanup_failed"));
    assert!(log.contains("provider_request_failed"));
    assert!(!log.contains("secret corrupt receipt"));
    assert!(!log.contains("secret native discovery"));
}
