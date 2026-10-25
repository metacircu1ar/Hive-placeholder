//! Behaviour regressions.
use super::*;

#[test]
fn gc_cli_error_reports_acknowledged_progress_and_failed_page_identity() {
    let stub = Stub::new(vec![
        gc_page(10, false, Some(json!({"ended_at":1,"id":"a"}))),
        json!({"error":{"code":"purge_cleanup_pending",
            "message":"retry"},
            "request_id":"daemon-retry"}),
    ]);
    let result = super::super::gc::run_before(stub.root(), 10, false);
    assert!(!result.success);
    assert_eq!(result.value["error"]["code"], "purge_cleanup_pending");
    assert_eq!(result.value["request_id"], "daemon-retry");
    assert_eq!(result.value["gc_progress"]["acknowledged_pages"], 1);
    assert_eq!(result.value["gc_progress"]["stats"]["deleted_agents"], 1);
    assert_eq!(result.value["gc_progress"]["complete"], false);
    assert_eq!(result.value["failed_page"]["may_have_committed"], true);
    let requests = stub.finish();
    assert_eq!(result.value["failed_page"]["request_id"], requests[1]["id"]);
    assert_eq!(result.value["failed_page"]["args"], requests[1]["args"]);
}

#[test]
fn delayed_cli_parses_duration_and_refuses_blocking_send() {
    use clap::Parser;
    let args = [
        "hivectl",
        "send",
        "a",
        "--body-file",
        "message.md",
        "--delay",
        "30m",
    ];
    let parsed = super::super::Cli::try_parse_from(args).unwrap();
    assert!(matches!(
        parsed.command,
        super::super::Command::Send(super::super::send::Send {
            delay: Some(1800),
            ..
        })
    ));
    let mut blocked = args.to_vec();
    blocked.push("--wait");
    assert!(super::super::Cli::try_parse_from(blocked).is_err());
}

#[test]
fn approval_poll_preserves_empty_operational_errors() {
    use clap::Parser;
    let failure = json!({"error":{"code":"provider_failed","message":""}});
    let stub = Stub::new(vec![failure.clone()]);
    let cli = super::super::Cli::try_parse_from([
        "hivectl",
        "--state-dir",
        stub.root().to_str().unwrap(),
        "approvals",
        "--wait",
        "--timeout",
        "1",
    ])
    .unwrap();
    let outcome =
        super::super::dispatch::execute(super::super::dispatch::Prepared::new(cli).unwrap())
            .unwrap();
    assert!(!outcome.success);
    assert!(outcome.input.is_none());
    assert_eq!(outcome.value, failure);
    assert_eq!(stub.finish().len(), 1);
}

#[test]
fn envelope_identity_is_not_mapped_to_an_artifact_id() {
    for (request, field) in [
        #[cfg(feature = "comb")]
        (
            json!({"op":"collector_admin", "action":"discard", "destination":"shared"}),
            Some("--request-id"),
        ),
        (json!({"op":"output"}), None),
        (json!({"op":"send"}), Some("--request-id")),
        #[cfg(feature = "comb")]
        (
            json!({"op":"evidence","kind":"correction","summary":"Reviewed correction."}),
            Some("--request-id"),
        ),
    ] {
        let error = runtime::api::InputContext::Control.error(
            hive_input::Code::InvalidRequestId,
            "/id",
            "a request ID",
        );
        let stub = Stub::new(vec![json!({"error":error})]);
        let result = remote_response(
            stub.root(),
            "retry",
            runtime::api::RequestEnvelope::new(request).request,
        );
        if let Some(field) = field {
            assert_eq!(result.unwrap().value["error"]["field"], field);
        } else {
            assert_eq!(result.err().unwrap().code(), "daemon_protocol_error");
        }
        stub.finish();
    }
}

#[test]
fn direct_body_rejections_keep_selected_file_context_without_rereading() {
    use clap::Parser;
    for (command, flag) in [
        ("spawn", "--body-file"),
        ("send", "--body-file"),
        ("complete", "--result-file"),
    ] {
        let error = runtime::api::InputContext::Send.error(
            hive_input::Code::InvalidValue,
            "/body",
            "a body string",
        );
        let stub = Stub::new(vec![json!({"error":error})]);
        let path = stub.root().join("PRIVATE-body.txt");
        fs::write(&path, "private body").unwrap();
        let mut args = vec![
            "hivectl",
            "--state-dir",
            stub.root().to_str().unwrap(),
            command,
        ];
        if command == "spawn" {
            args.extend(["--cwd", stub.root().to_str().unwrap()]);
        } else {
            args.push("a");
        }
        args.extend([flag, path.to_str().unwrap()]);
        let cli = super::super::Cli::try_parse_from(args).unwrap();
        let prepared = super::super::dispatch::Prepared::new(cli).unwrap();
        fs::remove_file(&path).unwrap();
        let outcome = super::super::dispatch::execute(prepared).unwrap();
        assert!(outcome.input.is_some());
        assert_eq!(outcome.value["error"]["field"], flag);
        assert!(!outcome.value.to_string().contains("PRIVATE"));
        assert!(!outcome.value.to_string().contains("private body"));
        assert_eq!(stub.finish()[0]["args"]["body"], "private body");
    }
}
