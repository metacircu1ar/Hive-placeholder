//! Requests regressions.
use super::*;

#[test]
fn daemon_response_status_requires_current_fields_and_known_enums() {
    let good = status_agent("completed", "failed", false);
    let mut invalid = vec![
        json!({}),
        json!({"agent":null}),
        json!({"agent":[]}),
        json!({"agent":17}),
        json!({"agent":good,"attachment_store_bytes":0,"extension":true}),
    ];
    for key in good.as_object().unwrap().keys() {
        let mut missing = good.clone();
        missing.as_object_mut().unwrap().remove(key);
        invalid.push(json!({"agent":missing,"attachment_store_bytes":0}));
    }
    for (key, value) in [
        ("state", json!("future")),
        ("task_outcome", json!("future")),
        ("state", json!({"failed":null})),
        ("task_outcome", json!({"completed":null})),
        ("expired", json!("true")),
        ("branch_hold", json!(null)),
        ("extension", json!(true)),
        ("graph_id", json!(null)),
        ("api_key_turns", json!(0)),
        ("provider", json!("claude")),
        ("billing", json!("api_key")),
        ("waiting_on", json!("robot")),
        ("approval_id", json!("unpaired")),
    ] {
        let mut agent = good.clone();
        agent[key] = value;
        invalid.push(json!({"agent":agent,"attachment_store_bytes":0}));
    }
    for response in invalid {
        let stub = Stub::new(vec![response.clone()]);
        let error = waiter(stub.root(), "completed").run().err().unwrap();
        assert_eq!(error.code(), "daemon_protocol_error", "{response}");
        assert_eq!(stub.finish().len(), 1);
    }
}

#[test]
fn daemon_response_accepts_current_status_enrichment_and_compact_fields() {
    let mut agent = status_agent("completed", "idle", false);
    agent["branch"] = json!({"source":"source","source_thread":"thread","source_execution":"exec",
        "source_turn":"turn","phase":"bound","deadline":null,"source_outcome":null,
        "source_handoff":null,"native_thread":"thread","error":null});
    agent["branch_dependents"] = json!(["child"]);
    agent["waiting_on"] = json!("human");
    agent["approval_id"] = json!("approval");
    agent["instructions"] = json!("current instructions");
    let response = json!({"agent":agent,
        "attachment_store_bytes":0,
        "steering_verification":[{"auth":"main",
        "model":"mock",

        "effort":"low","selection":"current_session","daemon_must_be_stopped":true,
        "enables_next_launch":true,"argv":["hivectl","verify"]}]});
    let stub = Stub::new(vec![response.clone()]);
    let result = waiter(stub.root(), "completed").run().unwrap();
    assert!(result.success);
    assert_eq!(result.value, response);
    assert_eq!(stub.finish().len(), 1);
}

#[test]
fn daemon_response_uses_valid_cursors_in_following_requests() {
    for cursor in [0, 42, u64::MAX] {
        let page = event_page(vec![], cursor);
        let stub = Stub::new(vec![page.clone(), page]);
        let mut waiter = waiter(stub.root(), "turn-finished");
        assert!(waiter.poll_events(&json!({})).unwrap().is_none());
        assert_eq!(waiter.after, cursor);
        assert!(waiter.poll_events(&json!({})).unwrap().is_none());
        assert_eq!(waiter.after, cursor);
        let requests = stub.finish();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0]["args"],
            json!({"op":"read","target":"agent-id","after":41,"limit":200})
        );
        assert_eq!(
            requests[1]["args"],
            json!({"op":"read","target":"agent-id","after":cursor,"limit":200})
        );
    }
}

#[test]
fn gc_cli_rejects_missing_unknown_and_nonobject_response_fields() {
    let good = gc_page(10, false, None);
    let mut malformed = vec![
        json!([10, false, {}, [], [], null, false]),
        json!({"error":null}),
        json!({"error":{"code":"x","message":"m"},"request_id":7}),
    ];
    for path in ["", "/stats"] {
        for key in good.pointer(path).unwrap().as_object().unwrap().keys() {
            let mut missing = good.clone();
            missing
                .pointer_mut(path)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(key);
            malformed.push(missing);
        }
        let mut unknown = good.clone();
        unknown.pointer_mut(path).unwrap()["unexpected"] = json!(true);
        malformed.push(unknown);
    }
    for (field, value) in [
        ("request_id", json!(7)),
        ("stats", json!([1, 1, 1, 0, 2, 0, 3, 0, 0, 6, 6])),
        ("blockers", json!([["a", "busy"]])),
        ("next_after", json!([1, "a"])),
    ] {
        let mut value_page = good.clone();
        value_page[field] = value;
        malformed.push(value_page);
    }
    for value in malformed {
        let stub = Stub::new(vec![value]);
        let result = super::super::gc::run_before(stub.root(), 10, false);
        assert!(!result.success);
        assert_eq!(result.value["error"]["code"], "daemon_protocol_error");
        assert_eq!(stub.finish().len(), 1);
    }
}

#[test]
fn typed_input_metadata_survives_sync_async_and_accepted_call_outcomes() {
    let failure = input_failure();
    let stub = Stub::new(vec![failure.clone()]);
    let response = runtime::client(stub.root(), "sync", Request::Ping).unwrap();
    assert_eq!(response, failure);
    stub.finish();
    let stub = Stub::new(vec![failure.clone()]);
    let error = crate::calls::executor()
        .unwrap()
        .block_on(crate::calls::exchange::<Value>(
            stub.root(),
            "async",
            &Request::Ping,
            tokio::time::Instant::now() + Duration::from_secs(1),
        ))
        .unwrap_err();
    assert!(error.input().is_some());
    assert_eq!(serde_json::to_value(error).unwrap(), failure["error"]);
    stub.finish();

    let stub = Stub::new(vec![
        json!({"message_id":"message","recipient":"agent","delivery_state":"accepted"}),
        failure.clone(),
    ]);
    let result = crate::calls::executor()
        .unwrap()
        .block_on(crate::calls::send(
            stub.root(),
            "retry",
            runtime::api::SendArgs {
                target: "agent".to_owned().into(),
                body: "work".to_owned().into(),
                mode: DeliveryMode::NextTurn.into(),
                attachments: runtime::api::Input::Missing,
                delay_seconds: runtime::api::Input::Missing,
            },
            1,
            |_| {},
        ));
    assert_eq!(
        result.outcome,
        runtime::api::CallOutcome::ObservationUnavailable
    );
    let outcome = super::super::CommandOutcome::from(result);
    assert!(outcome.input.is_none());
    assert_eq!(outcome.value["message_id"], "message");
    assert_eq!(outcome.value["request_id"], "retry");
    assert_eq!(outcome.value["error"], failure["error"]);
    stub.finish();

    let stub = Stub::new(vec![
        gc_page(10, false, Some(json!({"ended_at":1,"id":"a"}))),
        failure.clone(),
    ]);
    let outcome = super::super::gc::run_before(stub.root(), 10, false);
    assert!(outcome.input.is_none());
    assert_eq!(outcome.value["gc_progress"]["acknowledged_pages"], 1);
    assert_eq!(outcome.value["error"], failure["error"]);
    stub.finish();
}

#[test]
fn both_clients_reject_raw_duplicate_error_fields() {
    for raw in [
        r#"{"error":{"code":"provider_failed","code":"provider_failed","message":"x"}}"#,
        r#"{"error":{"code":"out_of_range","message":"x","field":"/limit","field":"/after","expected":"integer","examples":["hivectl agents"]}}"#,
        r#"{"error":{"code":"provider_failed","message":"x"},"error":{"code":"provider_failed","message":"x"}}"#,
    ] {
        let stub = Stub::raw(vec![Some(raw.into())]);
        assert_eq!(
            runtime::client(stub.root(), "sync", Request::Ping)
                .unwrap_err()
                .code(),
            "daemon_protocol_error"
        );
        stub.finish();
        let stub = Stub::raw(vec![Some(raw.into())]);
        let error = crate::calls::executor()
            .unwrap()
            .block_on(crate::calls::exchange::<Value>(
                stub.root(),
                "async",
                &Request::Ping,
                tokio::time::Instant::now() + Duration::from_secs(1),
            ))
            .unwrap_err();
        assert_eq!(error.code(), crate::error::ErrorCode::DaemonProtocol);
        stub.finish();
    }
}

#[test]
#[cfg(feature = "comb")]
fn evidence_remote_errors_keep_cli_source_and_request_identity_without_reopening() {
    use clap::Parser;
    for file_source in [false, true] {
        for (pointer, flag) in [
            (
                "/summary",
                if file_source {
                    "--summary-file"
                } else {
                    "--summary"
                },
            ),
            ("/files/1", "--file"),
            ("/commit", "--commit"),
            ("/verdict", "--verdict"),
            ("/kind", "KIND"),
        ] {
            let stub = Stub::new(vec![json!({"request_id":"evidence-retry", "error":{
                "code":"invalid_value", "message":"Invalid public evidence.", "field":pointer,
                "expected":"valid public evidence",
                     "examples":[r#"{"op":"evidence","kind":"review","summary":"Public","verdict":"findings"}"#]}})]);
            let source = stub.root().join("PRIVATE\nsummary");
            std::fs::write(&source, "public").unwrap();
            let parsed = super::super::Cli::try_parse_from([
                "hivectl",
                "--state-dir",
                stub.root().to_str().unwrap(),
                "evidence",
                "correction",
                if file_source {
                    "--summary-file"
                } else {
                    "--summary"
                },
                if file_source {
                    source.to_str().unwrap()
                } else {
                    "public"
                },
                "--request-id",
                "evidence-retry",
            ])
            .unwrap();
            let prepared = super::super::dispatch::Prepared::new(parsed).unwrap();
            std::fs::remove_file(source).unwrap();
            let response = super::super::dispatch::execute(prepared).unwrap();
            assert!(!response.success);
            assert!(response.input.is_some());
            assert_eq!(response.value["error"]["field"], flag);
            assert_eq!(response.value["request_id"], "evidence-retry");
            assert!(!response.value.to_string().contains("PRIVATE"));
            let requests = stub.finish();
            assert_eq!(requests[0]["id"], "evidence-retry");
            assert_eq!(requests[0]["args"]["summary"], "public");
            assert!(requests[0]["args"].get("verdict").is_none());
            assert!(requests[0]["args"].get("commit").is_none());
            assert!(!requests[0].to_string().contains("PRIVATE"));
        }
    }
}

#[test]
fn control_input_fields_map_to_real_cli_arguments_and_examples() {
    for (raw, pointer, field, command) in [
        (
            json!({"op":"status","target":"a"}),
            "/target",
            "TARGET",
            "status",
        ),
        (
            json!({"op":"status","target":"a"}),
            "/instructions",
            "--instructions",
            "status",
        ),
        (json!({"op":"spawn"}), "/body", "--body", "spawn"),
        (json!({"op":"spawn"}), "/from_agent", "--from", "spawn"),
        (json!({"op":"spawn"}), "/auth", "--auth", "spawn"),
        (json!({"op":"spawn"}), "/cwd", "--cwd", "spawn"),
        (json!({"op":"send"}), "/mode", "--mode", "send"),
        (
            json!({"op":"set_options"}),
            "/options/model",
            "--option",
            "set-options",
        ),
        (json!({"op":"set_auth"}), "/auth", "AUTH", "set-auth"),
        (json!({"op":"complete"}), "/body", "--body", "complete"),
        (json!({"op":"terminate"}), "/target", "TARGET", "terminate"),
        (json!({"op":"stop"}), "/target", "TARGET", "stop"),
        (json!({"op":"resume"}), "/target", "TARGET", "resume"),
        (json!({"op":"interrupt"}), "/target", "TARGET", "interrupt"),
        (json!({"op":"purge"}), "/recursive", "--recursive", "purge"),
        (json!({"op":"agents"}), "/auth", "--auth", "agents"),
        (
            json!({"op":"agents"}),
            "/stalled_after",
            "--stalled-after",
            "agents",
        ),
        (
            json!({"op":"batch"}),
            "/operations/2/body",
            "--operations-file",
            "batch",
        ),
        (
            json!({"op":"batch"}),
            "/operations/2/attachments",
            "--operations-file",
            "batch",
        ),
    ] {
        assert_control_input(raw, pointer, field, command);
    }
}

#[cfg(feature = "comb")]
#[test]
fn collector_input_fields_map_to_cli_arguments_and_examples() {
    for (raw, pointer, field, command) in [
        (
            json!({"op":"collector_admin", "action":"discard", "destination":"shared"}),
            "/destination",
            "DESTINATION",
            "collectors discard",
        ),
        (
            json!({"op":"collector_admin",
                 "action":"release_uncertain",
                 "destination":"shared",
                 "item":"item"}),
            "/item",
            "ITEM",
            "collectors release-uncertain",
        ),
        (
            json!({"op":"collector_admin",
                 "action":"rebind",
                 "destination":"shared",
                 "comb_id":"uuid"}),
            "/comb_id",
            "COMB_ID",
            "collectors rebind",
        ),
    ] {
        assert_control_input(raw, pointer, field, command);
    }
}

#[cfg(feature = "comb")]
#[test]
fn collector_remote_rejects_pointers_unused_by_the_requested_action() {
    for pointer in ["/item", "/comb_id", "/action", "/unknown"] {
        let error = runtime::api::InputContext::CollectorDiscard.error(
            hive_input::Code::InvalidValue,
            pointer,
            "a valid value",
        );
        let stub = Stub::new(vec![json!({"error":error})]);
        let request = runtime::api::RequestEnvelope::new(
            json!({"op":"collector_admin", "action":"discard", "destination":"shared"}),
        )
        .request;
        assert_eq!(
            remote_response(stub.root(), "retry", request)
                .err()
                .unwrap()
                .code(),
            "daemon_protocol_error"
        );
        stub.finish();
    }
}
