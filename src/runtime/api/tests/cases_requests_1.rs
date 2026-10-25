//! Requests regressions.
use super::*;

#[test]
fn request_id_rules_agree_for_hosted_tools_and_batch_items() {
    for (id, accepted) in [
        ("retry 1".into(), true),
        ("é".repeat(64), true),
        ("é".repeat(65), false),
        ("x".repeat(129), false),
        ("private\nidentity".into(), false),
        ("private\u{85}identity".into(), false),
        (String::new(), false),
    ] {
        let args = json!({"target":"a", "body":"hello", "request_id":id});
        assert_eq!(validate_tool("hive_send", &args).is_ok(), accepted);
        let mut item = args;
        item["op"] = json!("send");
        let decoded: BatchItem = serde_json::from_value(item.clone()).unwrap();
        assert_eq!(decoded.operation().is_ok(), accepted);
        assert_eq!(
            validate_tool(
                "hive_batch",
                &json!({"request_id":"outer", "operations":[item]})
            )
            .is_ok(),
            accepted
        );
    }
}

#[test]
fn tool_schemas_match_current_contract() {
    if std::env::var_os("HIVE_UPDATE_TOOL_SCHEMAS").is_some() {
        std::fs::write(
            concat!(env!("CARGO_MANIFEST_DIR"), "/src/runtime/api/").to_owned()
                + if cfg!(feature = "comb") {
                    "tool_schemas.json"
                } else {
                    "tool_schemas_core.json"
                },
            format!("{}\n", serde_json::to_string_pretty(tools()).unwrap()),
        )
        .unwrap();
        return;
    }
    let baseline: Value = serde_json::from_str(if cfg!(feature = "comb") {
        include_str!("../tool_schemas.json")
    } else {
        include_str!("../tool_schemas_core.json")
    })
    .unwrap();
    assert_eq!(tools(), &baseline);
}

#[test]
fn cli_request_serialization_preserves_nulls_omissions_and_batch_values() {
    let request = Request::Spawn(Box::new(SpawnArgs {
        role: "test".to_owned().into(),
        body: "body".to_owned().into(),
        cwd: PathBuf::from("/tmp").into(),
        auth: Input::nullable(None),
        model: Input::nullable(None),
        effort: Input::nullable(None),
        lifetime: Input::Missing,
        ..SpawnArgs::default()
    }));
    assert_eq!(
        serde_json::to_value(request).unwrap(),
        json!({
            "op": "spawn",
            "role": "test",
            "body": "body",
            "cwd": "/tmp",
            "auth": null,
            "model": null,
            "effort": null
        })
    );
    assert_eq!(
        serde_json::to_value(Request::Agents(AgentsArgs {
            after: 0.into(),
            limit: 200.into(),
            parent: Input::nullable(None),
            auth: Input::Missing,
            ..AgentsArgs::default()
        }))
        .unwrap(),
        json!({"op":"agents","after":0,"limit":200,"parent":null})
    );
    assert_eq!(
        serde_json::to_value(Request::Messages(MessagesArgs {
            target: "a".to_owned().into(),
            before: Input::nullable(None),
            limit: 100.into()
        }))
        .unwrap(),
        json!({"op":"messages","target":"a","before":null,"limit":100})
    );
    assert_eq!(
        serde_json::to_value(Request::Status(StatusArgs {
            target: "a".to_owned().into(),
            ..Default::default()
        }))
        .unwrap(),
        json!({"op":"status","target":"a"})
    );
    assert_eq!(
        serde_json::to_value(Request::Status(StatusArgs {
            target: "a".to_owned().into(),
            instructions: false.into()
        }))
        .unwrap(),
        json!({"op":"status","target":"a","instructions":false})
    );
    for operations in [
        json!(false),
        json!([{"op":"send","request_id":"x","extra":3.0}, null]),
    ] {
        let request = Request::Batch(BatchArgs {
            join: Input::Missing,
            operations: Input::from_value(operations.clone()),
        });
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({"op":"batch","operations":operations})
        );
    }
}

#[test]
fn tool_validation_accepts_current_per_operation_inputs() {
    for (name, args) in [
        (
            "hive_spawn",
            json!({"body":"task","request_id":"empty-instructions","instructions":""}),
        ),
        ("hive_agents", json!({"limit":0})),
        ("hive_read", json!({"target":"a","limit":0})),
        ("hive_messages", json!({"target":"a","limit":0,"before":0})),
        (
            "hive_spawn",
            json!({"role":" ","body":"body","request_id":"x"}),
        ),
        (
            "hive_batch",
            json!({
                "request_id": "outer",
                "operations": [
                    {
                        "op": "terminate",
                        "target": "a",
                        "request_id": "inner"
                    }
                ]
            }),
        ),
    ] {
        validate_tool(name, &args).unwrap();
    }
}

#[test]
fn replay_uses_original_fields_and_precedes_lifecycle_and_validation() {
    let temp = crate::runtime::test_directory();
    let mut core = core(temp.path());
    core.store().save(&agent("caller")).unwrap();
    core.store().save(&agent("recipient")).unwrap();
    let raw = json!({"op":"send","target":"recipient","body":"work","extra":{"retained":true}});
    let envelope = RequestEnvelope::new(raw.clone());
    assert_eq!(envelope.fingerprint(), raw.to_string());
    let accepted = call(&mut core, "caller", "send", raw.clone()).unwrap();
    let mut caller = core.store().agent("caller").unwrap();
    caller.task_outcome = TaskOutcome::Completed;
    core.store().save(&caller).unwrap();
    assert_eq!(
        call(&mut core, "caller", "send", raw.clone()).unwrap(),
        accepted
    );
    for changed in [
        json!({"op":"send","target":"recipient","body":"work"}),
        json!({
            "op": "send",
            "target": "recipient",
            "body": "work",
            "extra": {
                "retained": true
            },
            "mode": null
        }),
        json!({}),
    ] {
        assert_error(
            call(&mut core, "caller", "send", changed),
            Error::request_conflict("Request ID was already used with different arguments."),
        );
    }
    assert_error(
        call(
            &mut core,
            "caller",
            "new",
            json!({"op":"send","mode":"unknown"}),
        ),
        completed(),
    );
    // Read-only dispatch bypasses both the conflicting mutation ID and lifecycle.
    assert!(
        call(
            &mut core,
            "caller",
            "send",
            json!({"op":"status","target":"recipient"})
        )
        .is_ok()
    );
    assert_eq!(
        core.store().messages("recipient", None, 200).unwrap()["messages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn pagination_and_optional_control_fields_are_strict() {
    let temp = crate::runtime::test_directory();
    let mut core = core(temp.path());
    core.store().save(&agent("recipient")).unwrap();
    control_inputs::assert_input(
        call(
            &mut core,
            "human",
            "send",
            json!({"op":"send","target":"recipient","body":"work","mode":42}),
        ),
        "invalid_value",
        "/mode",
        "send",
    );
    assert_error(
        call(
            &mut core,
            "human",
            "bad-mode",
            json!({"op":"send","mode":"unknown"}),
        ),
        Error::unsupported_operation("Delivery mode must be next-turn, steer, or redirect."),
    );
    control_inputs::assert_input(
        call(
            &mut core,
            "human",
            "bad-fields",
            json!({"op":"send","mode":42}),
        ),
        "invalid_value",
        "/mode",
        "send",
    );
    for raw in [Value::Null, json!(-1), json!(1.0), json!("2"), json!(false)] {
        for request in [
            json!({"op":"agents","after":raw,"limit":raw,"parent":42}),
            json!({"op":"messages","target":"recipient","before":raw,"limit":raw}),
        ] {
            let error = call(&mut core, "human", "bad-page", request).unwrap_err();
            assert_eq!(error.code(), "invalid_value");
            assert!(serde_json::to_value(error).unwrap()["field"].is_string());
        }
    }
    control_inputs::assert_input(
        call(
            &mut core,
            "human",
            "agents",
            json!({"op":"agents","parent":42}),
        ),
        "invalid_value",
        "/parent",
        "agents",
    );
    control_inputs::assert_input(
        call(
            &mut core,
            "human",
            "status",
            json!({"op":"status","target":"recipient","instructions":"true"}),
        ),
        "invalid_value",
        "/instructions",
        "status",
    );
    let full = call(
        &mut core,
        "human",
        "status",
        json!({"op":"status","target":"recipient","instructions":true}),
    )
    .unwrap();
    assert_eq!(full["agent"]["instructions"], "original instructions");
}

#[test]
fn spawn_validation_order_and_late_body_failure_preserve_rollback() {
    let temp = crate::runtime::test_directory();
    let mut core = core(temp.path());
    core.config_mut().runtime.max_agents = 0;
    assert_error(
        call(&mut core, "human", "capacity", json!({"op":"spawn"})),
        Error::agent_limit("Configured nonterminal or draining agent capacity is exhausted."),
    );
    core.config_mut().runtime.max_agents = 128;
    assert_error(
        call(
            &mut core,
            "human",
            "role",
            json!({"op":"spawn","role":"absent"}),
        ),
        InputContext::Spawn.error(
            hive_input::Code::InvalidValue,
            "/role",
            "a configured role name",
        ),
    );
    control_inputs::configure_spawn(&mut core);
    assert_error(
        call(
            &mut core,
            "human",
            "cwd",
            json!({"op":"spawn","role":"test","cwd":"relative"}),
        ),
        InputContext::Spawn.error(
            hive_input::Code::InvalidPath,
            "/cwd",
            "an absolute UTF-8 directory path without NUL bytes",
        ),
    );
    assert_error(
        call(
            &mut core,
            "human",
            "instructions",
            json!({"op":"spawn","role":"test","cwd":temp.path()}),
        ),
        Error::instructions_unavailable("Cannot read a configured instruction file."),
    );
    std::fs::write(temp.path().join("role.md"), "instructions").unwrap();
    let no_body = json!({"op":"spawn","role":"test","cwd":temp.path()});
    control_inputs::assert_input(
        call(&mut core, "human", "body", no_body.clone()),
        "missing_argument",
        "/body",
        "spawn",
    );
    assert_eq!(core.store().active_count().unwrap(), 0);
    assert!(
        core.store()
            .prior("human", "body", &no_body.to_string())
            .unwrap()
            .is_none()
    );
    control_inputs::malformed_spawn_fields(&mut core, temp.path());
    let accepted = call(
        &mut core,
        "human",
        "body",
        json!({
            "op": "spawn",
            "role": "test",
            "cwd": temp.path(),
            "body": "work",
            "model": "mock"
        }),
    )
    .unwrap();
    let saved = core
        .store()
        .agent(accepted["agent_id"].as_str().unwrap())
        .unwrap();
    assert_eq!(saved.model, "mock");
    assert_eq!(saved.auth, "main");
    assert_eq!(saved.effort, "high");
}

#[test]
fn batch_validation_is_sequential_and_failed_batches_rollback_inner_replay() {
    let temp = crate::runtime::test_directory();
    let mut core = core(temp.path());
    core.store().save(&agent("caller")).unwrap();
    core.store().save(&agent("recipient")).unwrap();
    assert_batch_rollback(&mut core);
    assert_batch_operation_shape(&mut core);
}

#[test]
fn stalled_arguments_reject_bad_durations_cursors_and_present_filters() {
    for raw in [
        json!({"stalled_seconds":0}),
        json!({"stalled_seconds":-1}),
        json!({"stalled_seconds":"1"}),
        json!({"stalled_seconds":null}),
        json!({"stalled_seconds":u64::MAX}),
        json!({"stalled_seconds":1,"after":0}),
        json!({"stalled_seconds":1,"parent":null}),
        json!({"stalled_seconds":1,"parent":5}),
        json!({"stalled_seconds":1,"auth":"main"}),
        json!({"stalled_seconds":1,"limit":0}),
        json!({"stalled_seconds":1,"limit":"100"}),
        json!({"stalled_seconds":1,"stalled_after":null}),
        json!({"stalled_after":{"idle_since":1,"root_id":"a"}}),
        json!({"stalled_seconds":1,"stalled_after":{"idle_since":1,"root_id":""}}),
        json!({"stalled_seconds":1,"stalled_after":{"idle_since":1,"root_id":"a","extra":1}}),
        json!({"stalled_seconds":1,"stalled_after":{"idle_since":u64::MAX,"root_id":"a"}}),
    ] {
        let args: AgentsArgs = serde_json::from_value(raw.clone()).unwrap();
        assert!(args.stalled_requested());
        assert!(args.validate_stalled().is_err(), "{raw}");
    }
    let valid = json!({"stalled_seconds":1800,"stalled_after":{"idle_since":1,"root_id":"a"}});
    validate_tool("hive_agents", &valid).unwrap();
    assert_eq!(
        serde_json::from_value::<AgentsArgs>(valid)
            .unwrap()
            .validate_stalled()
            .unwrap(),
        1800
    );
    for cursor in [
        json!("1:a"),
        json!({"idle_since":1,"root_id":"a","extra":true}),
        json!({"idle_since":0,"root_id":""}),
    ] {
        assert!(
            validate_tool(
                "hive_agents",
                &json!({"stalled_seconds":1,"stalled_after":cursor})
            )
            .is_err()
        );
    }
}
