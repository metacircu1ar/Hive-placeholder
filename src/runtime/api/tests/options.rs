//! Durable desired settings do not mutate an active turn or its frozen baseline.
use super::*;
use crate::runtime::{api, core::preflight::Facts, domain::AgentState, provider::protocol::Model};

fn facts() -> Facts {
    let mut facts = Facts::default();
    facts.model_catalog = Some(vec![Model::parse(
        &json!({"model":"mock","supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"high"}]}),
    )]);
    facts
}
fn change(c: &mut Core, id: &str, options: Value) -> crate::error::Result<Value> {
    c.dispatch(
        &crate::runtime::hub::Caller::Human,
        id,
        &RequestEnvelope::new(json!({"op":"set_options","target":"a","options":options})),
        &facts(),
    )
}
#[test]
fn desired_options_are_durable_deduplicated_and_reset_against_the_spawn_baseline() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    let mut a = agent("a");
    a.state = AgentState::Running;
    a.applied_options_revision = Some(0);
    a.turn_selection = Some(a.desired_selection());
    let active = a.turn_selection.clone();
    let baseline = a.agent_options.baseline.clone();
    c.store().save(&a).unwrap();
    let request = json!({"effort":"high"});
    let accepted = change(&mut c, "change", request.clone()).unwrap();
    assert_eq!(change(&mut c, "change", request).unwrap(), accepted);
    let stored = c.store().agent("a").unwrap();
    assert_eq!(stored.options_revision, 1);
    assert_eq!(stored.applied_options_revision, Some(0));
    assert_eq!(stored.effort, "high");
    assert_eq!(stored.turn_selection, active);
    assert_eq!(stored.agent_options.baseline, baseline);
    assert_eq!(accepted["takes_effect"]["effort"], "next_turn");
    let events = c.store().events("a", 0, 100).unwrap();
    assert_eq!(events["events"].as_array().unwrap().len(), 1);
    assert_eq!(events["events"][0]["type"], "options_changed");
    change(&mut c, "reset", json!({"effort":null})).unwrap();
    let reset = c.store().agent("a").unwrap();
    assert_eq!(&reset.agent_options.desired(), baseline.as_ref());
    assert_eq!(reset.options_revision, 2);
    assert_eq!(reset.turn_selection, active);
    assert_eq!(
        reset.agent_options.public().options["effort"],
        baseline.values["effort"]
    );
}
#[test]
fn validation_failures_do_not_change_state_or_create_a_receipt() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    c.store().save(&agent("a")).unwrap();
    for (id, options) in [
        ("model", json!({"model":"unknown"})),
        ("effort", json!({"effort":"ultra"})),
        ("owned", json!({"codex.env":{"TOKEN":"secret"}})),
        ("wrong-provider", json!({"claude.max_output_tokens":1024})),
    ] {
        let error = change(&mut c, id, options).unwrap_err();
        assert!(
            matches!(error, crate::error::Error::Input { .. }),
            "{error}"
        );
        assert_eq!(c.store().agent("a").unwrap().options_revision, 0);
        assert!(
            c.store().events("a", 0, 100).unwrap()["events"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    for (id, config) in [
        ("native-model", "model = 'private-model'"),
        ("native-effort", "model_reasoning_effort = 'private-effort'"),
    ] {
        let error = c
            .dispatch(
                &crate::runtime::hub::Caller::Human,
                id,
                &RequestEnvelope::new(json!({"op":"set_options","target":"a","config":config})),
                &facts(),
            )
            .unwrap_err();
        let public = serde_json::to_string(&error).unwrap();
        assert!(!public.contains("private-"), "{public}");
        let stored = c.store().agent("a").unwrap();
        assert_eq!(stored.model, "mock");
        assert_eq!(stored.effort, "low");
        assert_eq!(stored.options_revision, 0);
        assert!(stored.agent_options.config.is_none());
        assert!(
            c.store().events("a", 0, 100).unwrap()["events"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}
#[test]
fn catalog_is_read_only_and_old_model_mutation_has_no_alias() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    let request = RequestEnvelope::new(
        json!({"op":"options_catalog","provider":"codex","model":"gpt-6-luna"}),
    );
    assert!(request.request.is_read_only());
    let result = c
        .dispatch(
            &crate::runtime::hub::Caller::Human,
            "catalog",
            &request,
            &Facts::default(),
        )
        .unwrap();
    assert_eq!(result["catalogs"].as_array().unwrap().len(), 1);
    assert!(
        !result["catalogs"][0]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["key"] == "codex.verbosity")
    );
    let old = RequestEnvelope::new(json!({"op":"set_model","target":"a","model":"mock"}));
    assert_eq!(
        c.dispatch(
            &crate::runtime::hub::Caller::Human,
            "old",
            &old,
            &Facts::default()
        )
        .unwrap_err()
        .code(),
        "unsupported_operation"
    );
}
#[test]
fn spawn_accepts_either_role_or_preset_and_validates_canonical_duplicates() {
    let args: SpawnArgs = serde_json::from_value(json!({"role":"one","preset":"two"})).unwrap();
    assert_eq!(
        args.options().err().unwrap().code(),
        "conflicting_arguments"
    );
    let args: SpawnArgs =
        serde_json::from_value(json!({"preset":"reviewer","options":{"effort":"low"}})).unwrap();
    let selected = args.options().unwrap();
    assert_eq!(selected.role, Some("reviewer"));
    assert_eq!(selected.options.unwrap()["effort"], "low");
    let config = crate::config::Config::parse(
        "auth_profiles: {main: {provider: codex, method: subscription}}\nproviders: {codex: {model: mock}}",
    )
    .unwrap();
    for native in [
        "model = 'private-model'",
        "model_reasoning_effort = 'private-effort'",
    ] {
        let args: SpawnArgs = serde_json::from_value(json!({"config":native})).unwrap();
        let Err(error) = args.resolve(&config) else {
            panic!("native model or effort was accepted")
        };
        let public = serde_json::to_string(&error).unwrap();
        assert!(!public.contains("private-"), "{public}");
    }
}

#[test]
fn children_inherit_effective_options_below_presets_without_pending_changes() {
    let config = crate::config::Config::parse("auth_profiles: {main: {provider: codex, method: subscription}}\nproviders: {codex: {model: mock}}\nroles: {reviewer: {options: {effort: low}}}").unwrap();
    let mut parent = agent("parent");
    parent.turn_selection = Some(parent.desired_selection());
    parent.agent_options = parent
        .agent_options
        .changed(
            crate::config::Provider::Codex,
            &serde_json::from_value(json!({"model":"pending-model"})).unwrap(),
            None,
        )
        .unwrap();
    parent.model = "pending-model".into();
    parent.options_revision = 1;
    let args: SpawnArgs = serde_json::from_value(json!({"preset":"reviewer"})).unwrap();
    let resolved = args.resolve_parent(&config, Some(&parent)).unwrap();
    assert_eq!(resolved.effort, "low");
    assert_eq!(
        resolved.model,
        parent.turn_selection.as_ref().unwrap().model
    );
    assert_ne!(resolved.model, "pending-model");
    assert_eq!(resolved.options.sources["model"].source, "inherited:parent");
    let error = validate_tool("hive_batch", &json!({"request_id":"batch", "operations":[{"op":"spawn","body":"work","request_id":"child","options":{"effort":"bad effort"}}]})).unwrap_err();
    let Error::Input { input, .. } = error else {
        panic!("input error")
    };
    assert_eq!(input.field(), "/operations/0/options/effort");
}

#[test]
fn process_options_are_admitted_atomically_and_share_canonical_native_identity() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    c.config_mut().runtime.max_process_variants = 1;
    c.store().save(&agent("a")).unwrap();
    c.store().save(&agent("peer")).unwrap();
    let before = c.store().agent("a").unwrap();
    assert_eq!(
        change(
            &mut c,
            "over-limit",
            json!({"codex.request_compression":true})
        )
        .unwrap_err()
        .code(),
        "invalid_value"
    );
    assert_eq!(
        c.store().agent("a").unwrap().options_revision,
        before.options_revision
    );
    assert!(
        c.store().events("a", 0, 100).unwrap()["events"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    c.config_mut().runtime.max_process_variants = 2;
    change(
        &mut c,
        "accepted",
        json!({"codex.request_compression":true}),
    )
    .unwrap();
    let response = c.dispatch(&crate::runtime::hub::Caller::Human, "same-native", &RequestEnvelope::new(json!({
        "op":"set_options","target":"peer","config":"[features]\nenable_request_compression = true"})), &facts()).unwrap();
    assert_eq!(
        response["takes_effect"]["codex.request_compression"],
        "process_move"
    );
    assert_eq!(
        c.store()
            .agent("a")
            .unwrap()
            .agent_options
            .desired()
            .process_key(crate::config::Provider::Codex)
            .unwrap(),
        c.store()
            .agent("peer")
            .unwrap()
            .agent_options
            .desired()
            .process_key(crate::config::Provider::Codex)
            .unwrap()
    );
    let config = crate::config::Config::parse("auth_profiles: {main: {provider: codex, method: subscription}}\nproviders: {codex: {model: mock}}").unwrap();
    let args: SpawnArgs =
        serde_json::from_value(json!({"options":{"codex.request_compression":true}})).unwrap();
    assert_eq!(
        args.resolve(&config).unwrap().options.values["codex.request_compression"],
        true
    );
}

#[test]
fn native_mutation_is_private_replaces_by_canonical_key_and_resets_to_baseline() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    c.store().save(&agent("a")).unwrap();
    let response = c
        .dispatch(
            &crate::runtime::hub::Caller::Human,
            "native",
            &RequestEnvelope::new(json!({"op":"set_options","target":"a",
                "config":"model_reasoning_summary = 'detailed'"})),
            &facts(),
        )
        .unwrap();
    assert_eq!(
        response.pointer("/desired/options/values/codex.reasoning_summary"),
        Some(&Value::Null)
    );
    let stored = c.store().agent("a").unwrap();
    let config = stored.agent_options.config.as_ref().unwrap();
    assert_eq!(config.content, "model_reasoning_summary = 'detailed'");
    let public = api::value(stored.public());
    let text = public.to_string();
    assert!(!text.contains("detailed"), "{text}");
    assert_eq!(
        public.pointer("/agent_options/native_configs/0/keys/0"),
        Some(&json!("codex.reasoning_summary"))
    );

    change(
        &mut c,
        "explicit",
        json!({"codex.reasoning_summary":"none"}),
    )
    .unwrap();
    let explicit = c.store().agent("a").unwrap();
    assert!(explicit.agent_options.config.is_none());
    assert_eq!(
        explicit
            .agent_options
            .desired()
            .string("codex.reasoning_summary"),
        Some("none")
    );
    change(&mut c, "reset", json!({"codex.reasoning_summary":null})).unwrap();
    assert!(
        !c.store()
            .agent("a")
            .unwrap()
            .agent_options
            .desired()
            .values
            .contains_key("codex.reasoning_summary")
    );
}

#[test]
fn every_public_selection_redacts_native_overlay_values() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    let mut a = agent("a");
    std::sync::Arc::make_mut(&mut a.agent_options.baseline)
        .values
        .insert("codex.reasoning_summary".into(), json!("detailed"));
    std::sync::Arc::make_mut(&mut a.agent_options.baseline)
        .sources
        .insert(
            "codex.reasoning_summary".into(),
            crate::options::OptionSource {
                source: "inherited:parent".into(),
                native_overlay: true,
            },
        );
    let selection = a.desired_selection();
    a.reserved_turn = Some(crate::runtime::domain::ReservedTurn {
        message_id: "m".into(),
        selection: selection.clone(),
    });
    a.session_selection = Some(selection.clone());
    a.last_accepted_selection = Some(selection.clone());
    a.turn_selection = Some(selection.clone());
    c.store().save(&a).unwrap();
    let public = api::value(a.public());
    for pointer in [
        "/reserved_turn/selection/options/values/codex.reasoning_summary",
        "/session_selection/options/values/codex.reasoning_summary",
        "/last_accepted_selection/options/values/codex.reasoning_summary",
        "/turn_selection/options/values/codex.reasoning_summary",
    ] {
        assert_eq!(public.pointer(pointer), Some(&Value::Null), "{pointer}");
    }
    let response = change(&mut c, "redacted", json!({"effort":"high"})).unwrap();
    let events = c.store().events("a", 0, 100).unwrap();
    let event = &events["events"][0]["data"];
    assert_eq!(
        event.pointer("/previous/options/values/codex.reasoning_summary"),
        Some(&Value::Null)
    );
    assert_eq!(
        event.pointer("/desired/options/values/codex.reasoning_summary"),
        Some(&Value::Null)
    );
    assert_eq!(
        response.pointer("/desired/options/values/codex.reasoning_summary"),
        Some(&Value::Null)
    );
    assert_eq!(
        c.store()
            .agent("a")
            .unwrap()
            .agent_options
            .baseline
            .string("codex.reasoning_summary"),
        Some("detailed")
    );
}

#[test]
fn hosted_options_map_validates_catalog_values_instead_of_rejecting_dynamic_keys() {
    assert!(validate_tool("hive_set_options",&json!({"target":"a","options":{"effort":"low","codex.reasoning_summary":"detailed"},"request_id":"change"})).is_ok());
    assert!(
        validate_tool(
            "hive_set_options",
            &json!({"target":"a","options":{"effort":null},"request_id":"reset"})
        )
        .is_ok()
    );
    assert!(
        validate_tool(
            "hive_spawn",
            &json!({"body":"work","options":{"codex.verbosity":null},"request_id":"spawn"})
        )
        .is_ok()
    );
    for options in [
        json!({"codex.env":{"TOKEN":"secret"}}),
        json!({"codex.reasoning_summary":"secret"}),
        json!({"unknown":"secret"}),
    ] {
        let error = validate_tool(
            "hive_set_options",
            &json!({"target":"a","options":options,"request_id":"invalid"}),
        )
        .unwrap_err();
        assert!(!serde_json::to_string(&error).unwrap().contains("secret"));
    }
}
