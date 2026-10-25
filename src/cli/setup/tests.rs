use super::*;
use clap::Parser;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

#[test]
fn missing_or_corrupt_configuration_never_matches_a_daemon_without_identity() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.yaml");
    let mut observation = daemon::Observation {
        product_version: env!("CARGO_PKG_VERSION").into(),
        binary_sha256: "0".repeat(64),
        commit: None,
        protocols: crate::compatibility::CONTROL_RANGE,
        agent_catalog: crate::compatibility::AGENT_CATALOG.current,
        state_format: crate::compatibility::STATE.current,
        configuration: None,
        resident_actors: 0,
        runtime_resources: daemon::Resources { active_turns: 0 },
    };
    assert!(!daemon::matches_configuration(&path, &observation));
    fs::write(&path, "logging: [broken]\n").unwrap();
    assert!(!daemon::matches_configuration(&path, &observation));
    fs::write(&path, "auth_profiles: {}\n").unwrap();
    assert!(!daemon::matches_configuration(&path, &observation));
    observation.configuration = crate::config::Config::load(&path).unwrap().loaded;
    assert!(observation.configuration.is_some());
    assert!(daemon::matches_configuration(&path, &observation));
    fs::write(&path, "auth_profiles: {}\n# changed\n").unwrap();
    assert!(!daemon::matches_configuration(&path, &observation));
}

#[test]
fn yaml_additions_preserve_comments_and_semantics_and_refuse_conflicts() {
    let original = "# operator choices\nauth_profiles:\n  codex-main: # owned by user\n    provider: codex\n    method: subscription\nproviders:\n  codex:\n    auth: codex-main # keep this\nruntime:\n  max_agents: 256\n";
    let wanted=config::strict_yaml("providers:\n  codex:\n    auth: codex-main\n    model: test-model\n    effort: low\ndefault_provider: codex\n").unwrap();
    let text = config::patch(original, &wanted).unwrap();
    assert!(text.contains("auth: codex-main # keep this"));
    assert!(text.starts_with("# operator choices\n"));
    let parsed = crate::config::Config::parse(&text).unwrap();
    let spawn = parsed
        .resolve_spawn(crate::config::SpawnOptions::default())
        .unwrap();
    assert_eq!(
        (spawn.model.as_str(), spawn.effort.as_str()),
        ("test-model", "low")
    );
    assert_eq!(parsed.runtime.max_agents, 256);
    assert_eq!(config::patch(&text, &wanted).unwrap(), text);
    assert!(config::patch("providers: {codex: {auth: codex-main}}\n", &wanted).is_err());
    assert!(config::patch("providers:\n  codex:\n    model: mine\n", &wanted).is_err());
    for invalid in [
        "login: skip\nlogin: browser\n",
        "login: &x skip\nprovider: *x\n",
        "<<: {}\n",
        "? [a, b]\n: value\n",
    ] {
        assert!(config::strict_yaml(invalid).is_err(), "{invalid}");
    }
}
#[test]
fn config_publication_preserves_symlink_and_backs_up_only_real_changes() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("actual.yaml");
    let link = temp.path().join("config.yaml");
    fs::write(&target, "auth_profiles: {}\n# preserve\n").unwrap();
    symlink(&target, &link).unwrap();
    let before = publication::Snapshot::inspect(&link).unwrap();
    assert!(before.publish(&before.text).unwrap().is_none());
    let updated = "auth_profiles: {}\n# preserve\ndefault_provider: codex\n";
    let backup = before.publish(updated).unwrap().unwrap();
    assert_eq!(fs::read_to_string(backup).unwrap(), before.text);
    assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(fs::read_to_string(&link).unwrap(), updated);
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let current = publication::Snapshot::inspect(&link).unwrap();
    assert!(current.publish(updated).unwrap().is_none());
    fs::write(&target, "auth_profiles: {}\n# concurrent edit\n").unwrap();
    assert!(current.publish(updated).is_err());
    let current = publication::Snapshot::inspect(&link).unwrap();
    fs::remove_file(&link).unwrap();
    let other = temp.path().join("other.yaml");
    fs::write(&other, &current.text).unwrap();
    symlink(&other, &link).unwrap();
    assert!(current.publish(updated).is_err());
}
#[test]
fn strict_answers_and_flags_resolve_explicit_opt_outs() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("answers.yaml");
    fs::write(&file,"provider: [claude]\nlogin: skip\nservices: true\nclaude-model: sonnet\nclaude-effort: low\n").unwrap();
    let cli = super::super::Cli::try_parse_from([
        "hivectl",
        "setup",
        "--answers",
        file.to_str().unwrap(),
        "--provider",
        "codex",
        "--no-services",
    ])
    .unwrap();
    let super::super::Command::Setup(args) = cli.command else {
        panic!("setup")
    };
    let values = args.resolve().unwrap().values;
    assert_eq!(
        values.provider.unwrap(),
        vec![crate::config::Provider::Codex]
    );
    assert_eq!(values.no_services, Some(true));
    assert_eq!(values.services, Some(false));
    assert_eq!(values.claude_model.as_deref(), Some("sonnet"));
    let cli = super::super::Cli::try_parse_from(["hivectl", "setup", "--services=false"]).unwrap();
    let super::super::Command::Setup(disabled) = cli.command else {
        panic!("setup")
    };
    assert_eq!(disabled.resolve().unwrap().values.no_services, Some(true));
    fs::write(&file, "answers: again.yaml\n").unwrap();
    assert!(args.resolve().is_err());
    fs::write(&file, "login: skip\nlogin: browser\n").unwrap();
    assert!(args.resolve().is_err());
}
#[test]
fn noninteractive_missing_choices_and_existing_conflicts_fail_readonly() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.yaml");
    let snapshot = publication::Snapshot::inspect(&path).unwrap();
    let options = options::Values::default();
    assert!(
        plan::Plan::inspect(&snapshot, &options, &mut terminal::Terminal::open(true))
            .err()
            .unwrap()
            .to_string()
            .contains("--provider")
    );
    let options = options::Values {
        provider: Some(vec![crate::config::Provider::Codex]),
        ..options
    };
    assert!(
        plan::Plan::inspect(&snapshot, &options, &mut terminal::Terminal::open(true))
            .err()
            .unwrap()
            .to_string()
            .contains("--login")
    );
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}
#[test]
fn loaded_identity_retains_the_exact_startup_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.yaml");
    fs::write(&path, "auth_profiles: {}\n").unwrap();
    let loaded = crate::config::Config::load(&path).unwrap();
    let previous = loaded.loaded.clone();
    fs::write(&path, "auth_profiles: {}\n# new\n").unwrap();
    assert_ne!(previous, crate::config::Config::load(&path).unwrap().loaded);
    assert_eq!(previous, loaded.loaded);
}

#[cfg(not(feature = "comb"))]
#[test]
fn core_rejects_positive_comb_setup_before_any_action() {
    for args in [
        vec!["--comb-root", "/missing"],
        vec!["--embedding-config", "/missing.yaml"],
        vec!["--collectors"],
        vec!["--no-comb-roles=false"],
    ] {
        let cli = super::super::Cli::try_parse_from(["hivectl", "setup"].into_iter().chain(args))
            .unwrap();
        let super::super::Command::Setup(options) = cli.command else {
            panic!("setup")
        };
        assert!(options.resolve().is_err());
    }
}

#[test]
fn intentional_manual_service_outcomes_do_not_hide_other_failures() {
    let p = plan::Plan {
        providers: vec![],
        default: crate::config::Provider::Codex,
        login: options::Login::Skip,
        additions: serde_yaml_ng::Value::Null,
    };
    let options = options::Values::default();
    let check = |name: &str, detail: serde_json::Value| {
        serde_json::from_value::<super::super::doctor::Check>(
            serde_json::json!({"name":name,"status":"error","detail":detail,"fixes":[]}),
        )
        .unwrap()
    };
    let unavailable = check("daemon", serde_json::json!({"code":"daemon_unavailable"}));
    assert!(expected_pending(&unavailable, &p, &options, true, None));
    assert!(!expected_pending(&unavailable, &p, &options, false, None));
    let stopped = check(
        "services",
        serde_json::json!({"ready":false,"missing_environment":[],"warnings":[],"failure":null}),
    );
    assert!(expected_pending(&stopped, &p, &options, true, None));
    for detail in [
        serde_json::json!({"ready":false,"missing_environment":["KEY"],"warnings":[],"failure":null}),
        serde_json::json!({"ready":false,"missing_environment":[],"warnings":[],"failure":{"code":"config_invalid"}}),
        serde_json::json!({"code":"state_io"}),
    ] {
        assert!(!expected_pending(
            &check("services", detail),
            &p,
            &options,
            true,
            None
        ));
    }
    let unavailable_comb = check(
        "comb:default",
        serde_json::json!({"credential_file":"/bridge-a","error":{"code":"comb_unavailable"}}),
    );
    assert!(!expected_pending(
        &unavailable_comb,
        &p,
        &options,
        true,
        None
    ));
    assert!(expected_pending(
        &unavailable_comb,
        &p,
        &options,
        false,
        Some(Path::new("/bridge-a"))
    ));
    assert!(!expected_pending(
        &unavailable,
        &p,
        &options,
        false,
        Some(Path::new("/bridge-a"))
    ));
    let other = check(
        "comb:role-other",
        serde_json::json!({"credential_file":"/bridge-b","error":{"code":"comb_unavailable"}}),
    );
    assert!(!expected_pending(
        &other,
        &p,
        &options,
        false,
        Some(Path::new("/bridge-a"))
    ));
}
#[test]
fn embedding_inputs_request_comb_explicitly() {
    for values in [
        options::Values {
            embedding_config: Some("/embedding.yaml".into()),
            ..Default::default()
        },
        options::Values {
            bridge_file: Some("/bridge.json".into()),
            ..Default::default()
        },
    ] {
        assert!(values.positive_comb());
    }
}

#[test]
fn guide_covers_every_visible_setup_flag() {
    use clap::CommandFactory;
    let command = crate::cli::Cli::command();
    let setup = command.find_subcommand("setup").unwrap();
    let guide = include_str!("../../../docs/setup.md");
    for arg in setup
        .get_arguments()
        .filter(|a| !a.is_hide_set() && !a.is_global_set())
    {
        if let Some(long) = arg.get_long() {
            assert!(
                guide
                    .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                    .any(|word| word == format!("--{long}")),
                "Undocumented setup flag --{long}"
            );
        }
    }
}

#[test]
fn login_method_uses_one_cli_and_answers_spelling() {
    use clap::ValueEnum;
    for value in options::Login::value_variants() {
        let possible = value.to_possible_value().unwrap();
        assert_eq!(serde_json::to_value(value).unwrap(), possible.get_name());
        assert_eq!(possible.get_name_and_aliases().count(), 1);
    }
}

#[test]
fn answers_origin_tracks_only_effective_values_and_flags_can_replace_bad_names() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("answers.yaml");
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .create_new(&path, b"codex-profile: invalid/name\n")
        .unwrap();
    let cli = crate::cli::Cli::try_parse_from([
        "hivectl",
        "setup",
        "--answers",
        path.to_str().unwrap(),
        "--codex-profile",
        "valid",
    ])
    .unwrap();
    let crate::cli::Command::Setup(args) = cli.command else {
        panic!("setup");
    };
    let resolved = args.resolve().unwrap();
    assert_eq!(resolved.values.codex_profile.as_deref(), Some("valid"));
    for flag in ["--codex-profile", "--login"] {
        let error = crate::cli::input::Context::new("setup", flag)
            .invalid(
                hive_input::Code::InvalidValue,
                "Invalid option.",
                "a valid selection",
            )
            .error;
        let error = resolved.attribute(error);
        let crate::error::Error::Input { input, .. } = error else {
            panic!("input");
        };
        assert_eq!(input.field(), flag);
    }
}

#[test]
fn answers_file_and_yaml_failures_retain_native_causes() {
    use std::error::Error as _;
    fn has<T: std::error::Error + 'static>(error: &crate::error::Error) -> bool {
        let mut source = error.source();
        while let Some(cause) = source {
            if cause.is::<T>() {
                return true;
            }
            source = cause.source();
        }
        false
    }
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("answers.yaml");
    let options = || {
        let cli = crate::cli::Cli::try_parse_from([
            "hivectl",
            "setup",
            "--answers",
            path.to_str().unwrap(),
        ])
        .unwrap();
        let crate::cli::Command::Setup(options) = cli.command else {
            panic!("setup");
        };
        options
    };
    assert!(has::<std::io::Error>(&options().resolve().err().unwrap()));
    for (bytes, yaml_error) in [
        (b"\xff".as_slice(), false),
        (b"login: skip\nlogin: browser\n".as_slice(), true),
    ] {
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .write(&path, bytes)
            .unwrap();
        let error = options().resolve().err().unwrap();
        if yaml_error {
            assert!(has::<serde_yaml_ng::Error>(&error));
        } else {
            assert!(has::<std::io::Error>(&error));
        }
    }
}

#[test]
fn reserved_resolved_config_target_is_rejected_before_setup_changes() {
    for name in [".hive-tmp-state-ABC123", ".comb-tmp-admin-ABC123"] {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join(name);
        let link = temp.path().join("config.yaml");
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .create_new(&target, b"auth_profiles: {}\n")
            .unwrap();
        symlink(&target, &link).unwrap();
        for path in [&target, &link] {
            assert!(publication::Snapshot::inspect(path).is_err());
        }
        assert_eq!(fs::read(&target).unwrap(), b"auth_profiles: {}\n");
        assert_eq!(fs::read_link(&link).unwrap(), target);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
    }
}
