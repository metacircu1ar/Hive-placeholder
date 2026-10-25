//! Transport regressions.
use super::*;

#[test]
fn message_call_transport_failure_keeps_acceptance_identity_for_resume() {
    let stub = Stub::new(vec![
        json!({"message_id":"message","recipient":"agent","delivery_state":"accepted"}),
        json!({"not_a_result":true}),
    ]);
    let result = crate::calls::executor()
        .unwrap()
        .block_on(crate::calls::send(
            stub.root(),
            "retry-key",
            runtime::api::SendArgs {
                attachments: crate::runtime::api::Input::Missing,
                delay_seconds: runtime::api::Input::Missing,
                target: "agent".to_owned().into(),
                body: "work".to_owned().into(),
                mode: DeliveryMode::NextTurn.into(),
            },
            1,
            |_| {},
        ));
    assert_eq!(
        result.outcome,
        runtime::api::CallOutcome::ObservationUnavailable
    );
    assert_eq!(result.message_id.as_deref(), Some("message"));
    assert_eq!(result.request_id.as_deref(), Some("retry-key"));
    let requests = stub.finish();
    assert_eq!(requests[0]["id"], "retry-key");
    assert!(requests[0]["args"].get("timeout").is_none());
    assert_eq!(requests[1]["args"]["op"], "message_result");
    assert_eq!(requests[1]["args"]["message_id"], "message");
}

#[test]
fn message_wait_timeout_keeps_pending_delivery_and_never_sends_cancel() {
    let stub = Stub::new(vec![message_snapshot("accepted", "pending")]);
    let result = crate::calls::executor()
        .unwrap()
        .block_on(crate::calls::wait(stub.root(), "message", 1, |_| {}));
    assert_eq!(result.outcome, runtime::api::CallOutcome::Timeout);
    assert_eq!(
        result.result.unwrap().delivery_state,
        runtime::domain::MessageState::Accepted
    );
    let requests = stub.finish();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["args"]["op"], "message_result");
}

#[test]
fn message_wait_deadline_bounds_unresponsive_socket_reads() {
    let root = tempfile::Builder::new()
        .prefix("hl-call-")
        .tempdir_in("/tmp")
        .unwrap();
    fs::create_dir(root.path().join("runtime")).unwrap();
    let listener = UnixListener::bind(runtime::socket_path(root.path())).unwrap();
    let worker = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        thread::sleep(Duration::from_secs(2));
        drop(stream);
    });
    let started = Instant::now();
    let result = crate::calls::executor()
        .unwrap()
        .block_on(crate::calls::wait(root.path(), "message", 1, |_| {}));
    assert_eq!(result.outcome, runtime::api::CallOutcome::Timeout);
    assert!(started.elapsed() < Duration::from_millis(1800));
    worker.join().unwrap();
}

#[test]
fn external_client_rejects_wrong_formats_and_malformed_error_envelopes() {
    for response in [
        json!({"unexpected":true,
            "artifact_id":"artifact",
            "offset":0,
            "text":"text",
            "next_offset":4,
            "bytes":4,
            "eof":true}),
        json!({"text":"missing required fields"}),
        json!({"error":null}),
        json!({"error":{"message":"missing code"}}),
    ] {
        let stub = Stub::new(vec![response]);
        let error = crate::calls::executor()
            .unwrap()
            .block_on(crate::calls::exchange::<runtime::api::OutputResponse>(
                stub.root(),
                "output",
                &Request::Ping,
                tokio::time::Instant::now() + Duration::from_secs(1),
            ))
            .unwrap_err();
        assert_eq!(error.code(), crate::error::ErrorCode::DaemonProtocol);
        stub.finish();
    }
}

#[test]
fn daemon_response_rejects_malformed_envelopes_and_preserves_valid_errors() {
    let response = json!({"error":{"code":"unknown_agent","message":"No agent has this ID."}});
    let stub = Stub::new(vec![response.clone()]);
    let result = remote_response(stub.root(), "request-id", Request::Ping).unwrap();
    assert!(!result.success);
    assert_eq!(result.value, response);
    assert_eq!(
        stub.finish(),
        vec![json!({"protocol":1,"id":"request-id","args":{"op":"ping"}})]
    );
    let stub = Stub::new(vec![response.clone()]);
    let result = waiter(stub.root(), "turn-finished").run().unwrap();
    assert!(!result.success);
    assert_eq!(result.value, response);
    assert_eq!(stub.finish().len(), 1);
    let mut malformed = vec![
        Value::Null,
        json!(false),
        json!(17),
        json!("raw"),
        json!([]),
        json!([{"error":null}]),
        json!({"error":{"code":"x","message":"m"},"extra":true}),
        json!({"error":{"code":"x","message":"m","extra":true}}),
    ];
    malformed.extend(
        [
            Value::Null,
            json!(false),
            json!(0),
            json!("failure"),
            json!([]),
            json!({}),
            json!({"code":"x"}),
            json!({"message":"m"}),
            json!(["x", "m"]),
        ]
        .into_iter()
        .map(|error| json!({"error":error})),
    );
    for response in malformed {
        let stub = Stub::new(vec![response.clone()]);
        let error = remote_response(stub.root(), "request-id", Request::Ping)
            .err()
            .unwrap();
        assert_eq!(error.code(), "daemon_protocol_error", "{response}");
        assert_eq!(stub.finish().len(), 1);
        let stub = Stub::new(vec![response.clone()]);
        let error = waiter(stub.root(), "turn-finished").run().err().unwrap();
        assert_eq!(error.code(), "daemon_protocol_error", "{response}");
        assert_eq!(stub.finish().len(), 1);
    }
}

#[test]
fn daemon_response_status_decisions_preserve_terminal_precedence() {
    for (outcome, state, expired, terminal) in [
        ("cancelled", "failed", true, Some(false)),
        ("completed", "failed", false, Some(true)),
        ("open", "failed", true, Some(true)),
        ("open", "failed", false, Some(false)),
        ("open", "idle", false, None),
    ] {
        let agent = status_agent(outcome, state, expired);
        let response = json!({"agent":agent,"attachment_store_bytes":0});
        let stub = Stub::new(vec![response.clone()]);
        let result = waiter(stub.root(), "completed").run().unwrap();
        if let Some(success) = terminal {
            assert_eq!(result.success, success);
            assert_eq!(result.value, response);
        } else {
            assert!(!result.success);
            assert_eq!(result.value, json!({"wait":"timeout","agent":agent}));
        }
        assert_eq!(stub.finish().len(), 1);
    }
}

#[test]
fn daemon_response_first_finished_event_precedes_terminal_agent_state() {
    let agent = status_agent("cancelled", "failed", true);
    let first = event("turn_finished", 17, json!({"unknown":[null,2]}));
    let events = event_page(
        vec![
            event("assistant_message", 16, json!({"raw":"provider"})),
            first.clone(),
            event("turn_finished", 18, json!({"other":true})),
        ],
        100,
    );
    let stub = Stub::new(vec![
        json!({"agent":agent,"attachment_store_bytes":0}),
        events,
    ]);
    let result = waiter(stub.root(), "turn-finished").run().unwrap();
    assert!(result.success);
    assert_eq!(result.value, json!({"agent":agent,"event":first}));
    assert_status_then_read(&stub.finish());
}

#[test]
fn daemon_response_event_errors_and_history_precede_matching_events() {
    let agent = status_agent("completed", "failed", false);
    let response = json!({"error":{"code":"unknown_agent","message":"No agent has this ID."}});
    let stub = Stub::new(vec![
        json!({"agent":agent,"attachment_store_bytes":0}),
        response.clone(),
    ]);
    let result = waiter(stub.root(), "turn-finished").run().unwrap();
    assert!(!result.success);
    assert_eq!(result.value, response);
    assert_status_then_read(&stub.finish());
    for pruned in [0, 21, u64::MAX] {
        let mut response = event_page(vec![event("turn_finished", 17, json!({}))], 100);
        response["history_expired"] = json!(true);
        response["pruned_through"] = json!(pruned);
        let stub = Stub::new(vec![
            json!({"agent":agent,"attachment_store_bytes":0}),
            response,
        ]);
        let result = waiter(stub.root(), "turn-finished").run().unwrap();
        assert!(!result.success);
        assert_eq!(
            result.value,
            json!({"error":{"code":"history_expired",
            "message":"Requested event history was compacted."},"pruned_through":pruned})
        );
        assert_status_then_read(&stub.finish());
    }
}

#[test]
fn daemon_response_rejects_malformed_events_without_advancing_cursor() {
    let good_event = event("turn_finished", 17, json!({}));
    let good_page = event_page(vec![good_event.clone()], 42);
    let mut malformed = vec![Value::Null, json!({}), json!([])];
    for key in good_page.as_object().unwrap().keys() {
        let mut missing = good_page.clone();
        missing.as_object_mut().unwrap().remove(key);
        malformed.push(missing);
    }
    for key in good_event.as_object().unwrap().keys() {
        let mut missing = good_event.clone();
        missing.as_object_mut().unwrap().remove(key);
        malformed.push(event_page(vec![missing], 42));
    }
    for field in ["cursor", "pruned_through"] {
        for value in [
            Value::Null,
            json!("7"),
            json!(-1),
            json!(7.0),
            json!(true),
            json!({}),
            json!([]),
        ] {
            let mut page = good_page.clone();
            page[field] = value;
            malformed.push(page);
        }
    }
    for (field, value) in [
        ("events", json!({})),
        ("events", Value::Null),
        ("events", json!("turn_finished")),
        ("history_expired", json!("true")),
        ("unexpected", json!(true)),
    ] {
        let mut page = good_page.clone();
        page[field] = value;
        malformed.push(page);
    }
    for bad in [
        Value::Null,
        json!(["turn_finished"]),
        json!({"type":"future"}),
        {
            let mut e = good_event.clone();
            e["type"] = json!({"turn_finished":null});
            e
        },
        {
            let mut e = good_event.clone();
            e["type"] = json!("future");
            e
        },
        {
            let mut e = good_event.clone();
            e["extension"] = json!(true);
            e
        },
    ] {
        // Invalid rows after a match must still be rejected.
        malformed.push(event_page(vec![good_event.clone(), bad], 42));
    }
    for response in malformed {
        let stub = Stub::new(vec![response.clone()]);
        let mut waiter = waiter(stub.root(), "turn-finished");
        let error = waiter.poll_events(&json!({})).err().unwrap();
        assert_eq!(error.code(), "daemon_protocol_error", "{response}");
        assert_eq!(waiter.after, 41);
        assert_eq!(stub.finish().len(), 1);
    }
}

#[test]
fn gc_cli_rejects_nonadvancing_cursor_instead_of_looping() {
    let next = Some(json!({"ended_at":1,"id":"a"}));
    let stub = Stub::new(vec![
        gc_page(10, false, next.clone()),
        gc_page(10, false, next),
    ]);
    let result = super::super::gc::run_before(stub.root(), 10, false);
    assert!(!result.success);
    assert_eq!(result.value["error"]["code"], "daemon_protocol_error");
    assert_eq!(stub.finish().len(), 2);
}

#[test]
fn stalled_cli_preserves_optional_presence_and_cursor() {
    use clap::Parser;
    let stub = Stub::new(vec![json!({"groups":[],"next_cursor":null})]);
    let parsed = super::super::Cli::try_parse_from([
        "hivectl",
        "--state-dir",
        stub.root().to_str().unwrap(),
        "agents",
        "--stalled",
        "30m",
        "--stalled-after",
        r#"{"idle_since":10,"root_id":"root"}"#,
    ])
    .unwrap();
    assert!(
        super::super::dispatch::execute(super::super::dispatch::Prepared::new(parsed).unwrap())
            .unwrap()
            .success
    );
    let requests = stub.finish();
    let args = &requests[0]["args"];
    assert_eq!(args["stalled_seconds"], 1800);
    assert_eq!(
        args["stalled_after"],
        json!({"idle_since":10,"root_id":"root"})
    );
    for key in ["after", "parent", "auth", "limit"] {
        assert!(args.get(key).is_none(), "{requests:?}");
    }
    for extras in [
        vec!["--after", "0"],
        vec!["--parent", "a"],
        vec!["--auth", "main"],
    ] {
        let mut args = vec!["hivectl", "agents", "--stalled", "30m"];
        args.extend(extras);
        assert!(super::super::Cli::try_parse_from(args).is_err());
    }
    assert!(
        super::super::Cli::try_parse_from([
            "hivectl",
            "agents",
            "--stalled-after",
            r#"{"idle_since":1,"root_id":"a"}"#
        ])
        .is_err()
    );
}

#[test]
fn join_wait_rejects_a_completed_result_for_another_identity() {
    let response = json!({"join_id":"wrong","requester":"human","request_id":"round","created_at":0,
        "deadline":null,"state":"completed","members":[]});
    let stub = Stub::new(vec![response]);
    let result = super::super::join_wait::run(stub.root(), "expected", 1).unwrap();
    assert!(!result.success);
    assert_eq!(result.value["wait"], "observation_unavailable");
    assert_eq!(result.value["join_id"], "expected");
    assert_eq!(result.value["error"]["code"], "daemon_protocol_error");
    assert_eq!(stub.finish()[0]["args"]["target"], "join:expected");
}

#[test]
fn approval_wait_keeps_bounded_polling_and_input_timeout_outcomes_distinct() {
    use clap::Parser;
    let error = json!({"error":{"code":"invalid_value",
        "message":"Invalid approval arguments.",
        "field":"/agent",
        "expected":"nonempty agent ID",
        "examples":[r#"{"op":"approvals","limit":100}"#]}});
    let stub = Stub::new(vec![error]);
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
    assert!(outcome.input.is_some());
    assert_eq!(outcome.value["error"]["field"], "--agent");
    let requests = stub.finish();
    assert_eq!(requests.len(), 1);
    assert_eq!(outcome.value["request_id"], requests[0]["id"]);
    let stub = Stub::raw_with_delay(vec![None], Duration::from_millis(1200));
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
    let start = Instant::now();
    let outcome =
        super::super::dispatch::execute(super::super::dispatch::Prepared::new(cli).unwrap())
            .unwrap();
    assert!(!outcome.success);
    assert!(outcome.input.is_none());
    assert_eq!(outcome.value["error"]["code"], "timeout");
    assert!(start.elapsed() < Duration::from_secs(3));
    let requests = stub.finish();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["args"]["op"], "approvals");
}
