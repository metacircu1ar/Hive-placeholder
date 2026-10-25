use super::*;
use crate::config::Provider;
use serde_json::json;

fn map(value: serde_json::Value) -> OptionMap {
    serde_json::from_value(value).unwrap()
}

#[test]
fn catalog_refuses_unknown_owned_wrong_provider_and_wrong_types_without_echoing_input() {
    let codex = catalog(Provider::Codex).unwrap();
    for input in [
        json!({"secret-token-that-is-not-an-option": "secret-value"}),
        json!({"claude.max_output_tokens": 1024}),
        json!({"codex.env": {"SECRET": "secret-value"}}),
        json!({"codex.request_compression": "secret-value"}),
        json!({"codex.auto_compact_token_limit": 1.5}),
        json!({"codex.auto_compact_token_limit": -1}),
        json!({"codex.auto_compact_token_limit": 2_000_001}),
        json!({"codex.reasoning_summary": "secret-value"}),
        json!({"thinking":"off"}),
        json!({"model":null}),
    ] {
        let error = codex.validate(&map(input), false).unwrap_err();
        let public = serde_json::to_string(&error).unwrap();
        assert!(!public.contains("secret"), "{public}");
        let crate::error::Error::Input { input, .. } = error else {
            panic!("expected typed input error")
        };
        assert_eq!(input.code(), hive_input::Code::InvalidValue);
    }
    assert!(codex.validate(&map(json!({"effort":null})), true).is_ok());
}

#[test]
fn capability_rules_use_resolved_model_and_validate_thinking_output_budget() {
    let claude = catalog(Provider::Claude).unwrap();
    let budget = map(json!({"thinking":2048}));
    assert!(
        claude
            .validate_capabilities(&budget, "claude-sonnet-4-6")
            .is_ok()
    );
    for model in ["sonnet", "claude-sonnet-5-5", "future-model"] {
        assert!(claude.validate_capabilities(&budget, model).is_err());
    }
    assert!(
        claude
            .validate_capabilities(&map(json!({"thinking":"on"})), "claude-sonnet-5-5")
            .is_ok()
    );
    assert!(
        claude
            .validate_capabilities(&map(json!({"thinking":"off"})), "claude-sonnet-5-5")
            .is_err()
    );
    assert!(
        claude
            .validate_capabilities(
                &map(json!({"thinking":2048,"claude.max_output_tokens":2048})),
                "claude-opus-4-6"
            )
            .is_err()
    );
    let codex = catalog(Provider::Codex).unwrap();
    assert!(
        codex
            .validate_capabilities(&map(json!({"codex.verbosity":"low"})), "gpt-5")
            .is_ok()
    );
    assert!(
        codex
            .validate_capabilities(&map(json!({"codex.verbosity":"low"})), "gpt-6-luna")
            .is_err()
    );
}

#[test]
fn precedence_and_reset_use_frozen_spawn_baseline_including_explicit_values() {
    let empty = OptionMap::new();
    let defaults =
        map(json!({"model":"provider-model","effort":"low","codex.reasoning_summary":"auto"}));
    let parent = Snapshot {
        values: map(
            json!({"model":"parent-model","effort":"medium","codex.reasoning_summary":"none"}),
        ),
        sources: std::collections::BTreeMap::default(),
    };
    let role = map(json!({"model":"role-model"}));
    let spawn = map(json!({"effort":"high"}));
    let snapshot = resolve(
        Provider::Codex,
        Some(Layer {
            source: "provider_default",
            options: &defaults,
            model: None,
            effort: None,
            overlay: None,
        }),
        Some((&parent, "parent")),
        Some(Layer {
            source: "role:worker",
            options: &role,
            model: None,
            effort: None,
            overlay: None,
        }),
        Layer {
            source: "spawn",
            options: &spawn,
            model: None,
            effort: None,
            overlay: None,
        },
    )
    .unwrap();
    assert_eq!(snapshot.string("model"), Some("role-model"));
    assert_eq!(snapshot.string("effort"), Some("high"));
    assert_eq!(snapshot.string("codex.reasoning_summary"), Some("none"));
    assert_eq!(
        snapshot.sources["codex.reasoning_summary"].source,
        "inherited:parent"
    );
    let mut overrides = empty;
    let changed = snapshot
        .changed(
            Provider::Codex,
            &mut overrides,
            &map(json!({"effort":"low","codex.tool_output_token_limit":1000})),
        )
        .unwrap();
    assert_eq!(changed.string("effort"), Some("low"));
    let reset = snapshot
        .changed(
            Provider::Codex,
            &mut overrides,
            &map(json!({"effort":null,"codex.tool_output_token_limit":null})),
        )
        .unwrap();
    assert_eq!(reset, snapshot);
    assert!(overrides.is_empty());
}

#[test]
fn shortcut_collisions_are_errors_and_invalid_changes_do_not_mutate_overrides() {
    let values = map(json!({"model":"same"}));
    assert!(
        resolve(
            Provider::Codex,
            None,
            None,
            None,
            Layer {
                source: "spawn",
                options: &values,
                model: Some("same"),
                effort: None,
                overlay: None,
            }
        )
        .is_err()
    );
    let base = Snapshot::default();
    let mut overrides = map(json!({"effort":"low"}));
    let before = overrides.clone();
    assert!(
        base.changed(
            Provider::Codex,
            &mut overrides,
            &map(json!({"codex.unknown":"bad"}))
        )
        .is_err()
    );
    assert_eq!(overrides, before);
}

#[test]
fn inherited_native_values_remain_redacted_until_replaced_by_non_native_value() {
    let parent = Snapshot {
        values: map(json!({"codex.reasoning_summary":"detailed"})),
        sources: [(
            "codex.reasoning_summary".into(),
            OptionSource {
                source: "spawn".into(),
                native_overlay: true,
            },
        )]
        .into(),
    };
    let empty = OptionMap::new();
    let inherited = resolve(
        Provider::Codex,
        None,
        Some((&parent, "a")),
        None,
        Layer {
            source: "spawn",
            options: &empty,
            model: None,
            effort: None,
            overlay: None,
        },
    )
    .unwrap();
    assert_eq!(
        inherited.public_values()["codex.reasoning_summary"],
        json!(null)
    );
    assert!(inherited.sources["codex.reasoning_summary"].native_overlay);
    assert!(!format!("{inherited:?}").contains("detailed"));
    assert!(!format!("{:?}", AgentOptions::new(inherited.clone())).contains("detailed"));
    let changed = inherited
        .changed(
            Provider::Codex,
            &mut OptionMap::new(),
            &map(json!({"codex.reasoning_summary":"none"})),
        )
        .unwrap();
    assert_eq!(
        changed.public_values()["codex.reasoning_summary"],
        json!("none")
    );
}

#[test]
fn spawn_can_clear_inherited_optional_capabilities_when_switching_models() {
    let parent = Snapshot {
        values: map(json!({"model":"gpt-5","effort":"low","codex.verbosity":"high"})),
        sources: std::collections::BTreeMap::default(),
    };
    let cleared = map(json!({"model":"gpt-6-luna","codex.verbosity":null}));
    let resolved = resolve(
        Provider::Codex,
        None,
        Some((&parent, "parent")),
        None,
        Layer {
            source: "spawn",
            options: &cleared,
            model: None,
            effort: None,
            overlay: None,
        },
    )
    .unwrap();
    assert!(!resolved.values.contains_key("codex.verbosity"));
    assert!(!resolved.sources.contains_key("codex.verbosity"));
    assert!(
        catalog(Provider::Codex)
            .unwrap()
            .validate_capabilities(&resolved.values, "gpt-6-luna")
            .is_ok()
    );
    for key in ["model", "effort"] {
        let invalid = [(key.to_owned(), serde_json::Value::Null)].into();
        assert!(
            resolve(
                Provider::Codex,
                None,
                Some((&parent, "parent")),
                None,
                Layer {
                    source: "spawn",
                    options: &invalid,
                    model: None,
                    effort: None,
                    overlay: None,
                }
            )
            .is_err()
        );
    }
}

#[test]
fn native_overlays_are_strict_catalog_mappings_with_redacted_debug_output() {
    let codex_text = concat!(
        "model_reasoning_summary = 'detailed'\n",
        "model_auto_compact_token_limit = 200000\n",
        "[features]\nenable_request_compression = true\n"
    );
    let codex = NativeConfig::parse(Provider::Codex, codex_text, "spawn")
        .unwrap()
        .unwrap();
    assert_eq!(codex.values["codex.reasoning_summary"], "detailed");
    assert_eq!(codex.values["codex.auto_compact_token_limit"], 200_000);
    assert_eq!(codex.values["codex.request_compression"], true);
    let debug = format!("{codex:?}");
    assert!(!debug.contains("detailed"), "{debug}");
    assert!(!debug.contains(codex_text), "{debug}");

    let claude = NativeConfig::parse(
        Provider::Claude,
        r#"{"alwaysThinkingEnabled":false,"autoCompactWindow":200000}"#,
        "spawn",
    )
    .unwrap()
    .unwrap();
    assert_eq!(claude.values["thinking"], "off");
    assert_eq!(claude.values["claude.auto_compact_window"], 200_000);
    assert!(
        NativeConfig::parse(Provider::Codex, "", "spawn")
            .unwrap()
            .is_none()
    );
    assert!(
        NativeConfig::parse(Provider::Claude, "{}", "spawn")
            .unwrap()
            .is_none()
    );

    for (provider, text, secret) in [
        (
            Provider::Claude,
            r#"{"autoCompactWindow":1,"autoCompactWindow":2}"#,
            "",
        ),
        (
            Provider::Claude,
            r#"{"unknown-secret":"private-value"}"#,
            "private-value",
        ),
        (
            Provider::Codex,
            "unknown_secret = 'private-value'",
            "private-value",
        ),
        (Provider::Codex, "model = 'private-model'", "private-model"),
        (
            Provider::Codex,
            "model_reasoning_effort = 'private-effort'",
            "private-effort",
        ),
        (
            Provider::Claude,
            r#"{"model":"private-model"}"#,
            "private-model",
        ),
        (
            Provider::Claude,
            r#"{"effortLevel":"private-effort"}"#,
            "private-effort",
        ),
        (Provider::Codex, "approval_policy = 'never'", ""),
    ] {
        let error = NativeConfig::parse(provider, text, "spawn").unwrap_err();
        let public = serde_json::to_string(&error).unwrap();
        assert!(!public.contains(secret) || secret.is_empty(), "{public}");
    }

    let oversized = "x".repeat(NATIVE_CONFIG_BYTES + 1);
    assert_eq!(
        NativeConfig::parse(Provider::Codex, &oversized, "spawn")
            .unwrap_err()
            .code(),
        "invalid_value"
    );
}

#[test]
fn stored_native_overlay_corruption_is_a_private_database_error() {
    let mut stored = NativeConfig::parse(
        Provider::Codex,
        "model_reasoning_summary = 'detailed'",
        "spawn",
    )
    .unwrap()
    .unwrap();
    stored.content = "private-broken-value = '''".into();
    let error = stored.validate_stored(Provider::Codex).unwrap_err();
    assert_eq!(error.code(), "database_error");
    let public = serde_json::to_string(&error).unwrap();
    assert!(!public.contains("private-broken-value"), "{public}");
}

#[test]
fn mutation_overlay_replacement_explicit_override_and_reset_are_deterministic() {
    let mut baseline = Snapshot::default();
    baseline.values = map(json!({
        "model":"gpt-5.6-luna",
        "effort":"high",
        "codex.reasoning_summary":"auto"
    }));
    baseline.sources = baseline
        .values
        .keys()
        .map(|key| (key.clone(), OptionSource::plain("spawn")))
        .collect();
    let runtime = NativeConfig::parse(
        Provider::Codex,
        "model_reasoning_summary = 'detailed'",
        "set_options",
    )
    .unwrap();
    let selected = AgentOptions::new(baseline.clone())
        .changed(Provider::Codex, &OptionMap::new(), Some(runtime))
        .unwrap();
    assert_eq!(
        selected.desired().string("codex.reasoning_summary"),
        Some("detailed")
    );
    assert!(selected.desired().sources["codex.reasoning_summary"].native_overlay);
    assert_eq!(
        selected.public().options["codex.reasoning_summary"],
        json!(null)
    );

    let explicit = selected
        .changed(
            Provider::Codex,
            &map(json!({"codex.reasoning_summary":"none"})),
            None,
        )
        .unwrap();
    assert_eq!(
        explicit.desired().string("codex.reasoning_summary"),
        Some("none")
    );
    assert!(!explicit.desired().sources["codex.reasoning_summary"].native_overlay);
    assert!(explicit.config.is_none());

    let reset = explicit
        .changed(
            Provider::Codex,
            &map(json!({"codex.reasoning_summary":null})),
            None,
        )
        .unwrap();
    assert_eq!(reset.desired(), baseline);
}

#[test]
fn a_layer_rejects_the_same_canonical_key_in_options_and_native_config() {
    let options = map(json!({"codex.reasoning_summary":"none"}));
    let overlay = NativeConfig::parse(
        Provider::Codex,
        "model_reasoning_summary = 'detailed'",
        "spawn",
    )
    .unwrap()
    .unwrap();
    assert!(
        resolve(
            Provider::Codex,
            None,
            None,
            None,
            Layer {
                source: "spawn",
                options: &options,
                model: None,
                effort: None,
                overlay: Some(&overlay),
            },
        )
        .is_err()
    );
}

#[test]
fn process_identity_hashes_only_sorted_canonical_process_values() {
    let mut direct = Snapshot::default();
    assert_eq!(direct.process_key(Provider::Codex).unwrap(), "");
    direct.values = map(json!({"codex.request_compression": true, "effort":"low"}));
    let native = NativeConfig::parse(
        Provider::Codex,
        "[features]\nenable_request_compression = true",
        "spawn",
    )
    .unwrap()
    .unwrap();
    let overlay = Snapshot {
        values: native.values,
        sources: std::collections::BTreeMap::default(),
    };
    assert_eq!(
        direct.process_key(Provider::Codex).unwrap(),
        overlay.process_key(Provider::Codex).unwrap()
    );
    direct
        .values
        .insert("codex.request_compression".into(), json!(false));
    assert_ne!(
        direct.process_key(Provider::Codex).unwrap(),
        overlay.process_key(Provider::Codex).unwrap()
    );
}
