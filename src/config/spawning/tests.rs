use super::*;

#[test]
fn sandbox_role_and_override_values_are_strict_and_default_stays_workspace_write() {
    let cfg = config(
        "default_provider: codex\nproviders: {codex: {model: mock}}\nroles: {wide: {sandbox: full-access}}\n",
    );
    assert_eq!(
        cfg.resolve_spawn(SpawnOptions::default()).unwrap().sandbox,
        Sandbox::WorkspaceWrite
    );
    assert_eq!(
        cfg.resolve_spawn(SpawnOptions {
            role: Some("wide"),
            ..SpawnOptions::default()
        })
        .unwrap()
        .sandbox,
        Sandbox::FullAccess
    );
    assert_eq!(
        cfg.resolve_spawn(SpawnOptions {
            role: Some("wide"),
            sandbox: Some(Sandbox::ReadOnly),
            ..SpawnOptions::default()
        })
        .unwrap()
        .sandbox,
        Sandbox::ReadOnly
    );
    for invalid in ["danger-full-access", "unrestricted", "full_access"] {
        assert!(serde_json::from_value::<Sandbox>(serde_json::json!(invalid)).is_err());
    }
}

#[test]
fn model_and_effort_syntax_agrees_with_cli_validators_and_keeps_catalog_checks_separate() {
    for model in ["sonnet[1m]", "provider/model:revision@alias", "gpt-6-luna"] {
        let cfg = config(&format!(
            "default_provider: codex\nproviders:\n  codex: {{model: {}}}\n",
            serde_json::to_string(model).unwrap()
        ));
        assert_eq!(
            cfg.resolve_spawn(SpawnOptions::default()).unwrap().model,
            model
        );
        assert!(hive_input::clap::parsers::model(model).is_ok());
    }
    for (model, effort) in [
        ("bad name", "high"),
        ("-bad", "high"),
        ("模型", "high"),
        ("sonnet", "fast"),
    ] {
        let yaml = format!(
            "auth_profiles:\n  main: {{provider: codex, method: subscription}}\nproviders:\n  codex: {{model: {}, effort: {}}}\n",
            serde_json::to_string(model).unwrap(),
            serde_json::to_string(effort).unwrap()
        );
        assert_eq!(Config::parse(&yaml).unwrap_err().code(), "config_invalid");
    }
}

fn config(extra: &str) -> Config {
    Config::parse(&format!("auth_profiles:\n  main: {{provider: codex, method: subscription}}\n  claude: {{provider: claude, method: subscription}}\n{extra}")).unwrap()
}

#[test]
fn defaults_role_and_explicit_overrides_have_declared_precedence() {
    let config = config(
        "default_provider: codex\nproviders:\n  codex: {auth: main, model: terra, effort: low}\n  claude: {auth: claude, model: sonnet, effort: medium}\nroles:\n  worker: {}\n  reviewer: {provider: claude}\n  special: {auth: main, model: sol, effort: high}\n",
    );
    let minimal = config.resolve_spawn(SpawnOptions::default()).unwrap();
    assert_eq!(
        (
            minimal.auth.as_str(),
            minimal.model.as_str(),
            minimal.effort.as_str()
        ),
        ("main", "terra", "low")
    );
    let reviewer = config
        .resolve_spawn(SpawnOptions {
            role: Some("reviewer"),
            ..SpawnOptions::default()
        })
        .unwrap();
    assert_eq!(
        (
            reviewer.provider,
            reviewer.model.as_str(),
            reviewer.effort.as_str()
        ),
        (Provider::Claude, "sonnet", "medium")
    );
    let explicit = config
        .resolve_spawn(SpawnOptions {
            role: Some("special"),
            model: Some("luna"),
            effort: Some("medium"),
            sandbox: Some(Sandbox::ReadOnly),
            ..SpawnOptions::default()
        })
        .unwrap();
    assert_eq!(
        (
            explicit.model.as_str(),
            explicit.effort.as_str(),
            explicit.sandbox
        ),
        ("luna", "medium", Sandbox::ReadOnly)
    );
    assert!(
        config
            .resolve_spawn(SpawnOptions {
                provider: Some(Provider::Claude),
                auth: Some("main"),
                ..SpawnOptions::default()
            })
            .is_err()
    );
}

#[test]
fn implicit_defaults_disappear_when_ambiguous_and_groups_are_never_implied() {
    let one=Config::parse("auth_profiles:\n  main: {provider: codex, method: subscription}\nproviders:\n  codex: {model: luna}\n").unwrap();
    assert_eq!(
        one.resolve_spawn(SpawnOptions::default()).unwrap().auth,
        "main"
    );
    let mut two = one.clone();
    two.auth_profiles
        .insert("second".into(), two.auth_profiles["main"].clone());
    two.auth_groups.insert(
        "both".into(),
        crate::config::AuthGroup::ordered(vec!["main".into(), "second".into()]),
    );
    let error = two.resolve_spawn(SpawnOptions::default()).err().unwrap();
    assert!(error.to_string().contains("providers.codex.auth"));
    assert_eq!(
        two.resolve_spawn(SpawnOptions {
            auth: Some("both"),
            ..SpawnOptions::default()
        })
        .unwrap()
        .auth,
        "both"
    );
    assert!(
        config("providers:\n  codex: {model: luna}\n")
            .resolve_spawn(SpawnOptions::default())
            .is_err()
    );
}

#[test]
fn groups_and_role_contradictions_are_rejected_at_config_load() {
    let base = "auth_profiles:\n  a: {provider: codex, method: subscription}\n  b: {provider: claude, method: subscription}\n";
    for extra in [
        "auth_groups: {g: {mode: ordered, members: []}}",
        "auth_groups: {g: {mode: ordered, members: [a, a]}}",
        "auth_groups: {g: {mode: ordered, members: [a, b]}}",
        "auth_groups: {g: {mode: ordered, members: [missing]}}",
        "auth_groups: {a: {mode: ordered, members: [a]}}",
        "roles: {r: {provider: claude, auth: a, model: luna}}",
    ] {
        assert!(Config::parse(&format!("{base}{extra}")).is_err(), "{extra}");
    }
    let mixed = "auth_profiles:\n  plan: {provider: codex, method: subscription}\n  key: {provider: codex, method: api_key, api_key_env: KEY}\n";
    assert!(
        Config::parse(&format!(
            "{mixed}auth_groups: {{g: {{mode: ordered, members: [plan], paid_tail: [key]}}}}\n"
        ))
        .is_ok()
    );
    assert!(
        Config::parse(&format!(
            "{mixed}auth_groups: {{g: {{mode: ordered, members: [key], paid_tail: [plan]}}}}\n"
        ))
        .is_err()
    );
}

#[test]
#[cfg(feature = "comb")]
fn comb_and_stdio_servers_resolve_with_bounded_role_overrides() {
    let cfg = config(
        "providers:\n  codex: {model: terra}\ncomb: {credential_file: /private/global.json}\nmcp_servers:\n  search: {command: global, args: [a], env: {TOKEN: secret}}\nroles:\n  worker:\n    provider: codex\n    comb: {credential_file: /private/reviewer.json, reviewer: true}\n    mcp_servers:\n      search: {command: role}\n      docs: {command: docs}\n",
    );
    let resolved = cfg
        .resolve_spawn(SpawnOptions {
            role: Some("worker"),
            ..Default::default()
        })
        .unwrap();
    assert!(resolved.comb.unwrap().reviewer);
    assert_eq!(resolved.mcp_servers["search"].command, "role");
    assert!(resolved.mcp_servers["search"].env.is_empty());
    assert_eq!(resolved.mcp_servers.len(), 2);
    for extra in [
        "comb: {credential_file: relative}",
        "mcp_servers: {hive: {command: no}}",
        "mcp_servers: {bad__name: {command: no}}",
        "mcp_servers: {'*': {command: no}}",
        "mcp_servers: {remote: {url: 'https://example.test'}}",
        "mcp_servers: {bad: {command: ''}}",
    ] {
        assert!(
            Config::parse(&format!(
                "auth_profiles: {{main: {{provider: codex, method: subscription}}}}\n{extra}\n"
            ))
            .is_err(),
            "{extra}"
        );
    }
}

#[test]
fn catalog_options_supply_model_and_effort_and_role_options_override_provider_defaults() {
    let cfg = config(
        "default_provider: codex\nproviders:\n  codex:\n    options: {model: provider-model, effort: low}\nroles:\n  reviewer:\n    options: {model: role-model}\n",
    );
    let resolved = cfg
        .resolve_spawn(SpawnOptions {
            role: Some("reviewer"),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(resolved.model, "role-model");
    assert_eq!(resolved.effort, "low");
    assert_eq!(resolved.options.sources["model"].source, "role:reviewer");
    for defaults in [
        "model: same, options: {model: same}",
        "options: {claude.max_output_tokens: 1024}",
        "options: {codex.env: {TOKEN: secret}}",
        "options: {codex.request_compression: null}",
    ] {
        assert!(Config::parse(&format!("auth_profiles: {{main: {{provider: codex, method: subscription}}}}\nproviders:\n  codex: {{{defaults}}}\n")).is_err());
    }
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("overlay.toml"),
        "model_reasoning_summary = 'detailed'",
    )
    .unwrap();
    let path = dir.path().join("config.yaml");
    fs::write(&path, "auth_profiles: {main: {provider: codex, method: subscription}}\nproviders: {codex: {model: mock}}\nroles: {reviewer: {config: overlay.toml}}")
        .unwrap();
    let config = Config::load(&path).unwrap();
    let resolved = config
        .resolve_spawn(SpawnOptions {
            role: Some("reviewer"),
            ..SpawnOptions::default()
        })
        .unwrap();
    assert_eq!(
        resolved.options.string("codex.reasoning_summary"),
        Some("detailed")
    );
    assert!(resolved.options.sources["codex.reasoning_summary"].native_overlay);
    assert_eq!(resolved.native_configs.len(), 1);
    fs::write(
        dir.path().join("overlay.toml"),
        "model_reasoning_summary = 'none'",
    )
    .unwrap();
    assert_eq!(
        resolved.options.string("codex.reasoning_summary"),
        Some("detailed")
    );
    let reread = config
        .resolve_spawn(SpawnOptions {
            role: Some("reviewer"),
            ..SpawnOptions::default()
        })
        .unwrap();
    assert_eq!(
        reread.options.string("codex.reasoning_summary"),
        Some("none")
    );
}
