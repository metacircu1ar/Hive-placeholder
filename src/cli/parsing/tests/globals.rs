use super::*;

#[test]
fn explicit_unused_globals_fail_before_or_after_commands_even_when_equal_to_defaults() {
    for (command, flags) in [
        (
            vec!["agents"],
            vec![
                ("--config", "PRIVATE"),
                ("--codex-bin", "codex"),
                ("--claude-bin", "claude"),
                ("--probe-timeout", "15"),
                ("--login-timeout", "600"),
            ],
        ),
        (
            vec!["auth", "list"],
            vec![("--codex-bin", "codex"), ("--probe-timeout", "15")],
        ),
        (vec!["auth", "groups"], vec![("--config", "PRIVATE")]),
        (
            vec!["auth", "generations", "main"],
            vec![("--config", "PRIVATE")],
        ),
        (vec!["daemon", "status"], vec![("--codex-bin", "codex")]),
        (vec!["daemon", "stop"], vec![("--claude-bin", "claude")]),
        (vec!["service", "start"], vec![("--codex-bin", "codex")]),
        (
            vec!["models", "--auth", "main"],
            vec![("--probe-timeout", "15")],
        ),
        (
            vec!["auth", "status", "main"],
            vec![("--login-timeout", "600")],
        ),
        (
            vec!["version"],
            vec![("--state-dir", "PRIVATE"), ("--config", "PRIVATE")],
        ),
        (
            vec!["spawn", "--cwd", "/work", "--body-file", "PRIVATE"],
            vec![("--managed-service", "PRIVATE")],
        ),
    ] {
        assert!(parse(&argv(&[vec!["hivectl"], command.clone()].concat()), false).is_ok());
        for (flag, value) in flags {
            for args in [
                [vec!["hivectl", flag, value], command.clone()].concat(),
                [vec!["hivectl"], command.clone(), vec![flag, value]].concat(),
            ] {
                let result = error(&args);
                assert_eq!(result["code"], "conflicting_arguments", "{args:?}");
                assert_eq!(result["field"], flag);
                assert!(!result.to_string().contains("PRIVATE"));
            }
        }
    }
}

#[test]
fn actual_global_consumers_accept_explicit_options() {
    for command in [
        vec!["setup"],
        vec!["doctor"],
        vec!["auth", "status", "main"],
        vec!["auth", "login", "main"],
        vec!["auth", "recover", "main"],
        vec!["auth", "repair", "main", "--reauthenticate"],
    ] {
        let args = [
            vec![
                "hivectl",
                "--config",
                "PRIVATE",
                "--state-dir",
                "/state",
                "--codex-bin",
                "codex",
                "--claude-bin",
                "claude",
                "--probe-timeout",
                "15",
            ],
            command,
        ]
        .concat();
        assert!(parse(&argv(&args), false).is_ok(), "{args:?}");
    }
    for command in [
        vec!["daemon", "run"],
        vec!["daemon", "start"],
        vec!["daemon", "restart"],
        vec!["service", "install"],
        vec!["models", "--auth", "main"],
    ] {
        let args = [
            vec![
                "hivectl",
                "--config",
                "PRIVATE",
                "--codex-bin",
                "codex",
                "--claude-bin",
                "claude",
            ],
            command,
        ]
        .concat();
        assert!(parse(&argv(&args), false).is_ok(), "{args:?}");
    }
    for command in [
        vec!["setup"],
        vec!["auth", "login", "main"],
        vec!["auth", "repair", "main", "--reauthenticate"],
    ] {
        let args = [vec!["hivectl", "--login-timeout", "600"], command].concat();
        assert!(parse(&argv(&args), false).is_ok(), "{args:?}");
    }
    for command in [
        vec!["daemon", "status"],
        vec!["daemon", "stop"],
        vec!["service", "start"],
        vec!["service", "status"],
        vec!["auth", "list"],
    ] {
        let args = [vec!["hivectl", "--config", "PRIVATE"], command].concat();
        assert!(parse(&argv(&args), false).is_ok(), "{args:?}");
    }
}

#[test]
fn facade_and_help_preserve_global_intent() {
    assert!(
        parse(
            &argv(&[
                "hive-daemon",
                "--config",
                "PRIVATE",
                "--codex-bin",
                "codex",
                "--managed-service",
                "PRIVATE"
            ]),
            true
        )
        .is_ok()
    );
    let failure = rejected(&["hive-daemon", "--probe-timeout", "15"], true);
    assert_eq!(failure.code, 2);
    assert!(failure.stderr.contains("hive-daemon --config"));
    for args in [
        vec!["hivectl", "--config", "PRIVATE", "agents", "--help"],
        vec!["hivectl", "--config", "PRIVATE", "--version"],
    ] {
        assert_eq!(rejected(&args, false).code, 0);
    }
    assert!(
        parse(
            &argv(&[
                "hivectl",
                "verify-steering",
                "--auth",
                "main",
                "--model",
                "sonnet",
                "--effort",
                "low",
                "--claude-bin",
                "claude"
            ]),
            false
        )
        .is_ok()
    );
    assert_eq!(
        error(&[
            "hivectl",
            "verify-steering",
            "--auth",
            "main",
            "--model",
            "sonnet",
            "--effort",
            "low",
            "--codex-bin",
            "codex"
        ])["field"],
        "--codex-bin"
    );
}

#[test]
fn generated_command_prefixes_parse_for_their_actual_consumers() {
    let cli = parse(&argv(&["hivectl", "doctor"]), false).ok().unwrap();
    for (path, tail) in [
        ("spawn", "spawn --cwd . --body Work"),
        ("service start", "service start"),
        ("service status", "service status"),
        ("daemon stop", "daemon stop"),
        ("daemon start", "daemon start"),
        ("daemon run", "daemon run"),
        ("setup", "setup --restart-daemon"),
        ("auth login", "auth login main"),
        ("auth status", "auth status main"),
        ("auth recover", "auth recover main"),
    ] {
        let prefix = crate::cli::doctor::command(
            &cli,
            std::path::Path::new("/config"),
            std::path::Path::new("/state"),
            path,
        );
        let text = format!("{prefix} {tail}");
        // Fixture paths contain no whitespace; shell quoting itself is unchanged.
        let args: Vec<_> = text
            .split_whitespace()
            .map(|s| OsString::from(s.trim_matches('\'')))
            .collect();
        if let Err(exit) = parse(&args, false) {
            panic!("{text}: {}", exit.stderr);
        }
    }
}

#[test]
fn daemon_start_retains_explicit_default_provider_choices() {
    let omitted = parse(&argv(&["hivectl", "daemon", "start"]), false)
        .ok()
        .unwrap();
    assert_eq!(omitted.explicit_provider, None);
    for (flag, value) in [("--codex-bin", "codex"), ("--claude-bin", "claude")] {
        for args in [
            vec!["hivectl", flag, value, "daemon", "start"],
            vec!["hivectl", "daemon", "start", flag, value],
        ] {
            let cli = parse(&argv(&args), false).ok().unwrap();
            assert_eq!(cli.explicit_provider, Some(flag));
        }
    }
}
#[test]
fn all_declared_globals_are_classified_for_uninstall() {
    let schema = super::schema(false);
    let actual: std::collections::BTreeSet<_> = schema
        .get_arguments()
        .filter(|arg| arg.is_global_set())
        .map(|arg| arg.get_id().as_str())
        .collect();
    let declared = super::super::globals::GLOBALS;
    let expected: std::collections::BTreeSet<_> = declared.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        declared.len(),
        expected.len(),
        "duplicate global classification"
    );
    assert_eq!(actual, expected);
    for (_, flag) in declared {
        let error = super::super::globals::removal(&[flag], false).unwrap_err();
        assert_eq!(error.code(), "conflicting_arguments", "{flag}");
    }
}
