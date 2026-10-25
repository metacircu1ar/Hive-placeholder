use super::*;

pub(super) fn assert_input(result: crate::error::Result<Value>, code: &str, field: &str, op: &str) {
    let error = api_error(result.unwrap_err());
    assert_eq!(error["code"], code);
    assert_eq!(error["field"], field);
    assert!(!error["expected"].as_str().unwrap().is_empty());
    let example: Value = serde_json::from_str(error["examples"][0].as_str().unwrap()).unwrap();
    assert_eq!(example["op"], op);
}
fn api_error(error: Error) -> Value {
    let value = serde_json::to_value(error).unwrap();
    assert!(value.get("received").is_none());
    assert!(!value.to_string().contains("private-canary"));
    value
}

#[test]
fn deferred_text_distinguishes_missing_type_and_byte_limits() {
    use std::error::Error as _;
    for (input, code) in [
        (Input::Missing, "missing_argument"),
        (
            Input::from_value(json!({"private-canary":false})),
            "invalid_value",
        ),
        (Input::from_value(json!(null)), "invalid_value"),
        (Input::Valid(String::new()), "out_of_range"),
        (Input::Valid("é".repeat(32769)), "out_of_range"),
    ] {
        let error = input.required(InputContext::Send, "/body").unwrap_err();
        if matches!(input, Input::Invalid(_)) {
            assert!(error.source().is_some());
        }
        let error = api_error(error);
        assert_eq!(error["code"], code);
        assert_eq!(error["field"], "/body");
    }
    let value: Input<bool> = Input::Missing;
    assert!(
        value
            .optional(InputContext::Status, "/instructions", "a boolean")
            .unwrap()
            .is_none()
    );
}

#[test]
fn direct_reads_identify_missing_and_malformed_targets() {
    let temp = crate::runtime::test_directory();
    let c = core(temp.path());
    for (op, field) in [
        ("status", "target"),
        ("read", "target"),
        ("messages", "target"),
        ("output", "id"),
        ("message_result", "message_id"),
        ("auth_availability", "auth"),
    ] {
        for (value, code) in [
            (None, "missing_argument"),
            (Some(json!(null)), "invalid_value"),
            (Some(json!({"private-canary":true})), "invalid_value"),
            (Some(json!("")), "out_of_range"),
        ] {
            let mut raw = json!({"op":op});
            if let Some(value) = value {
                raw[field] = value;
            }
            assert_input(pagination::observe(&c, raw), code, &format!("/{field}"), op);
        }
    }
}

#[test]
fn optional_read_fields_are_not_silently_dropped() {
    let temp = crate::runtime::test_directory();
    let c = core(temp.path());
    c.store().save(&agent("a")).unwrap();
    for (op, field) in [
        ("agents", "parent"),
        ("agents", "auth"),
        ("status", "instructions"),
    ] {
        for value in [
            json!(null),
            json!(42),
            json!({"private-canary":true}),
            json!([]),
        ] {
            let mut raw = json!({"op":op,"target":"a"});
            raw[field] = value;
            assert_input(
                pagination::observe(&c, raw),
                "invalid_value",
                &format!("/{field}"),
                op,
            );
        }
    }
    assert_input(
        pagination::observe(&c, json!({"op":"agents","parent":"unknown"})),
        "invalid_value",
        "/parent",
        "agents",
    );
    assert!(pagination::observe(&c, json!({"op":"status","target":"a"})).is_ok());
    assert!(pagination::observe(&c, json!({"op":"messages","target":"a","before":null})).is_ok());
}

#[test]
fn spawn_options_reject_malformed_values_instead_of_using_defaults() {
    for field in [
        "role",
        "from_agent",
        "instructions",
        "auth",
        "model",
        "effort",
        "provider",
        "sandbox",
        "lifetime",
        "api_fallback",
    ] {
        for value in [
            json!(null),
            json!(42),
            json!({"private-canary":true}),
            json!([]),
        ] {
            let mut raw = json!({});
            raw[field] = value;
            let args: SpawnArgs = serde_json::from_value(raw).unwrap();
            let error = args.options().err().unwrap();
            let error = api_error(error);
            assert_eq!(error["code"], "invalid_value", "{field}");
            assert_eq!(error["field"], format!("/{field}"));
        }
    }
    let args: SpawnArgs = serde_json::from_value(json!({"instructions":""})).unwrap();
    assert!(args.options().is_ok());
}

#[test]
fn wrong_type_send_modes_never_enqueue_but_accepted_replay_still_wins() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    c.store().save(&agent("a")).unwrap();
    for (index, value) in [
        json!(null),
        json!(42),
        json!(false),
        json!([]),
        json!({"private-canary":true}),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("accepted-{index}");
        let raw = json!({"op":"send","target":"a","body":"work","mode":value});
        assert_input(
            call(&mut c, "human", "new", raw.clone()),
            "invalid_value",
            "/mode",
            "send",
        );
        // A durable accepted response is authoritative even if an argument's rule tightens.
        let accepted = json!({"message_id":"accepted-before-rule-change"});
        c.store()
            .remember("human", &id, &raw.to_string(), &accepted)
            .unwrap();
        assert_eq!(call(&mut c, "human", &id, raw).unwrap(), accepted);
    }
    assert!(
        c.store().messages("a", None, 100).unwrap()["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn sequential_batch_field_errors_keep_index_and_rollback() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    c.store().save(&agent("a")).unwrap();
    let raw = json!({"op":"batch","operations":[
        {"op":"send","request_id":"one","target":"a","body":"work"},
        {"op":"send","request_id":"two","target":"a","body":false},
        {"op":"send","request_id":"three","target":false}
    ]});
    assert_input(
        call(&mut c, "human", "batch", raw),
        "invalid_value",
        "/operations/1/body",
        "batch",
    );
    assert!(
        c.store().messages("a", None, 100).unwrap()["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

pub(super) fn malformed_spawn_fields(core: &mut Core, cwd: &Path) {
    for field in ["auth", "model", "effort", "cwd"] {
        let mut raw = json!({"op":"spawn","role":"test","cwd":cwd,"body":"work"});
        raw[field] = json!(false);
        control_inputs::assert_input(
            call(core, "human", "malformed", raw),
            "invalid_value",
            &format!("/{field}"),
            "spawn",
        );
        assert_eq!(core.store().active_count().unwrap(), 0);
    }
}

pub(super) fn configure_spawn(core: &mut Core) {
    let profile: Profile =
        serde_json::from_value(json!({"provider":"codex","method":"subscription"})).unwrap();
    core.config_mut()
        .auth_profiles
        .insert("main".into(), profile.clone());
    let role: Role =
        serde_json::from_value(json!({"auth":"main","model":"mock","instructions":"role.md"}))
            .unwrap();
    core.config_mut().roles.insert("test".into(), role);
    core.state().register("main", &profile).unwrap();
}

#[test]
fn malformed_cwd_cannot_be_replaced_with_the_parent_directory() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    configure_spawn(&mut c);
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let mut parent = agent("parent");
    parent.cwd = workspace;
    c.store().save(&parent).unwrap();
    let inherited =
        json!({"op":"spawn","role":"test","body":"work","instructions":"","sandbox":"read-only"});
    let accepted = call(&mut c, "parent", "inherit", inherited.clone()).unwrap();
    let child = c
        .store()
        .agent(accepted["agent_id"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        child.cwd,
        c.store()
            .agent("parent")
            .unwrap()
            .cwd
            .canonicalize()
            .unwrap()
    );
    let count = c.store().active_count().unwrap();
    let mut invalid = inherited;
    invalid["cwd"] = json!(false);
    assert_input(
        call(&mut c, "parent", "invalid", invalid),
        "invalid_value",
        "/cwd",
        "spawn",
    );
    assert_eq!(c.store().active_count().unwrap(), count);
}
