//! Branch selection must use the same canonical precedence as ordinary spawns.
use super::*;
use crate::runtime::test_support::AgentBuilder;
use serde_json::json;

#[test]
fn spawn_widening_enabled_does_not_allow_changing_a_branch_source_mode() {
    let mut config = Config::parse("auth_profiles: {main: {provider: codex, method: subscription}}\nproviders: {codex: {model: mock}}\nroles: {wide: {sandbox: full-access}}\n").unwrap();
    config.runtime.allow_spawn_widening = true;
    let source = Source {
        agent: AgentBuilder::new("source")
            .configure(|a| a.sandbox = crate::config::Sandbox::ReadOnly)
            .build(),
        instructions: String::new(),
    };
    let args: SpawnArgs = serde_json::from_value(json!({"role":"wide","body":"work"})).unwrap();
    assert_eq!(
        source.resolve(&config, &args).unwrap().sandbox,
        crate::config::Sandbox::ReadOnly
    );
    let args: SpawnArgs =
        serde_json::from_value(json!({"body":"work","sandbox":"full-access"})).unwrap();
    match source.resolve(&config, &args) {
        Err(error) => assert_eq!(error.code(), "binding_conflict"),
        Ok(_) => panic!("a branch changed its source sandbox"),
    }
}

#[test]
fn branches_retain_source_sandbox_and_reject_changes() {
    let config = Config::parse("auth_profiles: {main: {provider: codex, method: subscription}}\nproviders: {codex: {model: mock}}\nroles: {restricted: {sandbox: read-only}}\n").unwrap();
    for sandbox in [
        crate::config::Sandbox::ReadOnly,
        crate::config::Sandbox::WorkspaceWrite,
        crate::config::Sandbox::FullAccess,
    ] {
        let source = Source {
            agent: AgentBuilder::new("source")
                .configure(|a| a.sandbox = sandbox)
                .build(),
            instructions: String::new(),
        };
        for role in [None, Some("restricted")] {
            let mut raw = json!({"body":"work"});
            if let Some(role) = role {
                raw["role"] = json!(role);
            }
            let args: SpawnArgs = serde_json::from_value(raw.clone()).unwrap();
            assert_eq!(source.resolve(&config, &args).unwrap().sandbox, sandbox);
            for override_mode in [
                crate::config::Sandbox::ReadOnly,
                crate::config::Sandbox::WorkspaceWrite,
                crate::config::Sandbox::FullAccess,
            ] {
                raw["sandbox"] = json!(override_mode);
                let args: SpawnArgs = serde_json::from_value(raw.clone()).unwrap();
                assert_eq!(
                    source.resolve(&config, &args).is_ok(),
                    override_mode == sandbox
                );
            }
        }
    }
}

#[test]
fn role_option_and_shortcut_forms_beat_source_and_explicit_arguments_beat_role() {
    let config = Config::parse("auth_profiles: {main: {provider: codex, method: subscription}}\nproviders: {codex: {model: default-model}}\nroles:\n  mapped: {options: {model: role-model, effort: low}}\n  shortcut: {model: role-model, effort: low}\n").unwrap();
    let source = Source {
        agent: AgentBuilder::new("source")
            .configure(|a| {
                a.model = "source-model".into();
                a.effort = "high".into();
            })
            .build(),
        instructions: String::new(),
    };
    for role in ["mapped", "shortcut"] {
        let args: SpawnArgs = serde_json::from_value(json!({"role":role,"body":"work"})).unwrap();
        let resolved = source.resolve(&config, &args).unwrap();
        assert_eq!(resolved.model, "role-model");
        assert_eq!(resolved.effort, "low");
        let args: SpawnArgs = serde_json::from_value(
            json!({"role":role,"body":"work","model":"explicit-model","effort":"medium"}),
        )
        .unwrap();
        let resolved = source.resolve(&config, &args).unwrap();
        assert_eq!(resolved.model, "explicit-model");
        assert_eq!(resolved.effort, "medium");
    }
    let args: SpawnArgs = serde_json::from_value(json!({"body":"work"})).unwrap();
    let resolved = source.resolve(&config, &args).unwrap();
    assert_eq!(resolved.model, "source-model");
    assert_eq!(resolved.effort, "high");
    assert_eq!(resolved.options.sources["model"].source, "inherited:source");
}
