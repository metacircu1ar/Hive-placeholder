use super::*;
use crate::{
    config::{Config, Provider},
    runtime::core::preflight::{Facts, RequestInput},
};
use control_inputs::assert_input;

fn configured(c: &mut Core) {
    control_inputs::configure_spawn(c);
    c.config_mut().auth_profiles.insert(
        "claude".into(),
        Profile {
            provider: Provider::Claude,
            method: crate::config::Method::Subscription,
            api_key_env: None,
            api_key_file: None,
        },
    );
}

fn args(raw: Value) -> SpawnArgs {
    serde_json::from_value(raw).unwrap()
}

#[test]
fn explicit_spawn_selections_fail_before_config_or_file_work() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    for (raw, code, field) in [
        (
            json!({"model":"private-canary model"}),
            "invalid_model",
            "/model",
        ),
        (
            json!({"effort":"private-canary"}),
            "invalid_effort",
            "/effort",
        ),
        (
            json!({"role":"private-canary role"}),
            "invalid_value",
            "/role",
        ),
        (
            json!({"auth":"private-canary login"}),
            "invalid_value",
            "/auth",
        ),
        (json!({"role":"private-canary"}), "invalid_value", "/role"),
        (json!({"auth":"private-canary"}), "invalid_value", "/auth"),
        (
            json!({"model":"bad model","api_fallback":42}),
            "invalid_value",
            "/api_fallback",
        ),
    ] {
        let mut raw = raw;
        raw["op"] = json!("spawn");
        assert_input(
            call(&mut c, "human", "rejected", raw.clone()),
            code,
            field,
            "spawn",
        );
        assert!(
            c.store()
                .prior("human", "rejected", &raw.to_string())
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(c.store().active_count().unwrap(), 0);
    assert_eq!(
        args(json!({"model":"valid"}))
            .resolve(c.config())
            .err()
            .unwrap()
            .code(),
        "config_invalid"
    );
}

#[test]
fn caller_provider_conflicts_do_not_change_auth_override_precedence() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    configured(&mut c);
    for raw in [
        json!({"provider":"claude","auth":"main"}),
        json!({"provider":"claude","role":"test"}),
    ] {
        let error = args(raw).resolve(c.config()).err().unwrap();
        assert_input(Err(error), "conflicting_arguments", "/provider", "spawn");
    }
    let resolved = args(json!({"role":"test","auth":"claude","model":"sonnet"}))
        .resolve(c.config())
        .unwrap();
    assert_eq!(resolved.provider, Provider::Claude);
    assert_eq!(resolved.auth, "claude");
    for model in ["sonnet[1m]", "provider/model:revision@alias"] {
        assert!(
            args(json!({"role":"test","model":model}))
                .resolve(c.config())
                .is_ok()
        );
    }
    c.config_mut().roles.get_mut("test").unwrap().model = Some("broken config model".into());
    assert_eq!(
        args(json!({"role":"test"}))
            .resolve(c.config())
            .err()
            .unwrap()
            .code(),
        "config_invalid"
    );
    c.config_mut().auth_groups.insert(
        "broken".into(),
        crate::config::AuthGroup::ordered(vec!["absent".into()]),
    );
    assert_eq!(
        args(json!({"auth":"broken"}))
            .resolve(c.config())
            .err()
            .unwrap()
            .code(),
        "unknown_profile"
    );
}

#[test]
fn branch_options_distinguish_explicit_input_from_inherited_model_and_effort() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    configured(&mut c);
    let mut source = crate::runtime::branching::Source {
        agent: agent("source"),
        instructions: String::new(),
    };
    for (model, effort) in [
        ("broken inherited model", "high"),
        ("mock", "broken-effort"),
    ] {
        source.agent.model = model.into();
        source.agent.effort = effort.into();
        assert_eq!(
            source
                .resolve(c.config(), &args(json!({})))
                .err()
                .unwrap()
                .code(),
            "config_invalid"
        );
    }
    let error = source
        .resolve(c.config(), &args(json!({"effort":"private-canary"})))
        .err()
        .unwrap();
    assert_input(Err(error), "invalid_effort", "/effort", "spawn");
    let error = source
        .resolve(c.config(), &args(json!({"role":"private-canary"})))
        .err()
        .unwrap();
    assert_input(Err(error), "invalid_value", "/role", "spawn");
    source.agent.effort = "high".into();
    assert_eq!(
        source
            .resolve(c.config(), &args(json!({"auth":"claude"})))
            .err()
            .unwrap()
            .code(),
        "binding_conflict"
    );
    assert!(Config::parse("auth_profiles: {main: {provider: codex, method: subscription}}\nroles: {bad: {auth: main, model: 'broken model'}}").is_err());
}

#[test]
fn conclusive_spawn_catalog_errors_follow_original_override_fields() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    configured(&mut c);
    for (overrides, field) in [
        (json!({"model":"private-canary"}), "/model"),
        (json!({"effort":"ultra"}), "/effort"),
        (json!({"model":"mock"}), "/model"),
    ] {
        c.config_mut().roles.get_mut("test").unwrap().effort = Some("medium".into());
        let mut raw =
            json!({"op":"spawn","role":"test","cwd":temp.path(),"instructions":"","body":"work"});
        raw.as_object_mut()
            .unwrap()
            .extend(overrides.as_object().unwrap().clone());
        assert_input(
            call(&mut c, "human", "miss", raw),
            "invalid_value",
            field,
            "spawn",
        );
    }
    c.config_mut().roles.get_mut("test").unwrap().model = Some("not-in-catalog".into());
    let raw = json!({"op":"spawn","role":"test","cwd":temp.path(),"instructions":"","body":"work"});
    assert_eq!(
        call(&mut c, "human", "inherited", raw.clone())
            .unwrap_err()
            .code(),
        "unknown_model"
    );
    assert_eq!(c.store().active_count().unwrap(), 0);
}

#[test]
fn same_code_discovery_failure_is_not_a_conclusive_input_error() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    configured(&mut c);
    let request = RequestEnvelope::new(
        json!({"op":"spawn","role":"test","model":"unknown","cwd":temp.path(),"instructions":"","body":"work"}),
    );
    let caller = crate::runtime::hub::Caller::Human;
    let mut facts = RequestInput::capture(&c, &caller, &request).resolve();
    for spawn in facts.spawns.values_mut() {
        for member in &mut spawn.members {
            member.catalog = Some(Err(Error::unknown_model(
                "Requested model is absent from the provider catalog.",
            )
            .with_source(std::io::Error::other("private-canary"))));
        }
    }
    let error = c
        .dispatch(&caller, "discovery", &request, &facts)
        .unwrap_err();
    assert_eq!(error.code(), "unknown_model");
    assert!(
        !serde_json::to_string(&error)
            .unwrap()
            .contains("private-canary")
    );
}

#[test]
fn set_options_commit_checks_catalog_with_explicit_and_retained_effort_provenance() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    c.store().save(&agent("a")).unwrap();
    let mut facts = Facts::default();
    facts.model_catalog = Some(vec![crate::runtime::provider::protocol::Model::parse(
        &json!({"model":"mock","supportedReasoningEfforts":[{"reasoningEffort":"high"}]}),
    )]);
    for (extra, code, field) in [
        (
            json!({"model":"private-canary model"}),
            "invalid_value",
            "/options/model",
        ),
        (
            json!({"model":"private-canary"}),
            "invalid_value",
            "/options/model",
        ),
        (
            json!({"model":"mock","effort":"private-canary"}),
            "invalid_value",
            "/options/effort",
        ),
        (
            json!({"model":"mock","effort":"ultra"}),
            "invalid_value",
            "/options/effort",
        ),
        (json!({"model":"mock"}), "invalid_value", "/options/model"),
    ] {
        let mut raw = json!({"options":extra});
        raw["op"] = json!("set_options");
        raw["target"] = json!("a");
        assert_input(
            c.dispatch(
                &crate::runtime::hub::Caller::Human,
                "model",
                &RequestEnvelope::new(raw),
                &facts,
            ),
            code,
            field,
            "set_options",
        );
        assert_eq!(c.store().agent("a").unwrap().options_revision, 0);
    }
    let mut a = c.store().agent("a").unwrap();
    a.agent_options
        .overrides
        .insert("effort".into(), json!("corrupt-effort"));
    let selection: SetOptionsArgs =
        serde_json::from_value(json!({"target":"a","options":{"model":"mock"}})).unwrap();
    assert_eq!(selection.selection(&a).unwrap_err().code(), "invalid_value");
}

#[test]
fn selection_rejections_preserve_replay_and_target_authority() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    configured(&mut c);
    c.store().save(&agent("a")).unwrap();
    for (op, extra) in [
        ("set_auth", json!({"auth":"claude"})),
        ("set_options", json!({"options":{"model":"bad model"}})),
    ] {
        let mut raw = extra;
        raw["op"] = json!(op);
        raw["target"] = json!("missing");
        assert_eq!(
            call(&mut c, "human", "missing", raw.clone())
                .unwrap_err()
                .code(),
            "unknown_agent"
        );
        raw["target"] = json!("a");
        let key = format!("accepted-{op}");
        let accepted = json!({"agent_id":"a"});
        c.store()
            .remember("human", &key, &raw.to_string(), &accepted)
            .unwrap();
        assert_eq!(call(&mut c, "human", &key, raw).unwrap(), accepted);
    }
    assert_input(
        call(
            &mut c,
            "human",
            "bad-auth",
            json!({"op":"set_auth","target":"a","auth":"claude"}),
        ),
        "conflicting_arguments",
        "/auth",
        "set_auth",
    );
    assert_input(
        call(
            &mut c,
            "human",
            "unknown-auth",
            json!({"op":"set_auth","target":"a","auth":"private-canary"}),
        ),
        "invalid_value",
        "/auth",
        "set_auth",
    );
}

#[test]
fn ended_targets_precede_invalid_model_and_login_arguments() {
    let temp = crate::runtime::test_directory();
    let mut c = core(temp.path());
    let mut ended = agent("ended");
    ended.task_outcome = TaskOutcome::Completed;
    c.store().save(&ended).unwrap();
    for raw in [
        json!({"op":"set_auth","target":"ended","auth":"private-canary"}),
        json!({"op":"set_options","target":"ended","options":{"model":"private-canary model"}}),
    ] {
        assert_eq!(
            call(&mut c, "human", "ended", raw).unwrap_err().code(),
            "agent_completed"
        );
    }
}
