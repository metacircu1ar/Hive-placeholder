//! Transport regressions.
use super::*;

#[test]
fn approval_preparation_preserves_cursor_options_and_remote_response_flags() {
    use clap::Parser;
    let stub = Stub::new(vec![json!({"approvals":[],"next_after":null})]);
    let cli = super::super::Cli::try_parse_from([
        "hivectl",
        "--state-dir",
        stub.root().to_str().unwrap(),
        "approvals",
        "--all",
        "--after",
        r#"{"created_at":7,"id":"opaque"}"#,
        "--limit",
        "200",
    ])
    .unwrap();
    let prepared = super::super::dispatch::Prepared::new(cli).unwrap();
    assert!(super::super::dispatch::execute(prepared).unwrap().success);
    let requests = stub.finish();
    assert_eq!(
        requests[0]["args"],
        json!({"op":"approvals","all":true,"after":{"created_at":7,"id":"opaque"},"limit":200})
    );
    for action in ["--accept", "--decline", "--cancel"] {
        let stub = Stub::new(vec![json!({"request_id":"retry",
                 "error":{"code":"invalid_value",
                "message":"Invalid action.",
                "field":"/action",
                "expected":"supported action",
                "examples":["hivectl respond APPROVAL_ID --accept"]}})]);
        let cli = super::super::Cli::try_parse_from([
            "hivectl",
            "--state-dir",
            stub.root().to_str().unwrap(),
            "respond",
            "00000000-0000-4000-8000-000000000001",
            action,
            "--request-id",
            "retry",
        ])
        .unwrap();
        let result =
            super::super::dispatch::execute(super::super::dispatch::Prepared::new(cli).unwrap())
                .unwrap();
        assert_eq!(result.value["error"]["field"], action);
        assert_eq!(result.value["request_id"], "retry");
        assert!(result.input.is_some());
        stub.finish();
    }
}

#[test]
fn direct_wait_input_errors_keep_wait_context_and_exit_two() {
    for event_read in [false, true] {
        let error = runtime::api::InputContext::Status.error(
            hive_input::Code::InvalidValue,
            "/target",
            "an agent ID",
        );
        let stub = Stub::new(vec![json!({"error":error})]);
        let outcome = if event_read {
            waiter(stub.root(), "turn-finished")
                .poll_events(&json!({}))
                .unwrap()
                .unwrap()
        } else {
            waiter(stub.root(), "completed").run().unwrap()
        };
        let input = outcome.input.unwrap();
        assert_eq!(input.value()["error"]["field"], "TARGET");
        let rendered =
            serde_json::from_value::<hive_input::InputError>(input.value()["error"].clone())
                .unwrap()
                .render(hive_input::Product::Hive)
                .unwrap();
        assert_eq!(rendered.exit_code, 2);
        assert!(
            input.value()["error"]["examples"][0]
                .as_str()
                .unwrap()
                .starts_with("hivectl wait ")
        );
        stub.finish();
    }
}
