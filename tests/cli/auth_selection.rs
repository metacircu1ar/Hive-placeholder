//! Selection mistakes precede native setup without hiding repair corruption.
use super::{Fixture, Provider, ordering};
use serde_json::Value;
use std::process::Output;

pub(super) fn input_error(output: Output, field: &str, command: &str) {
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr.clone()).unwrap();
    let value = ordering::error(output, "invalid_value");
    let error = &value["error"];
    assert_eq!(error["field"], field);
    assert!(!error["expected"].as_str().unwrap().is_empty());
    assert!(error.get("received").is_none());
    assert!(stderr.contains(field));
    let examples = error["examples"].as_array().unwrap();
    assert!(!examples.is_empty());
    for example in examples {
        let example = example.as_str().unwrap();
        assert!(example.starts_with(command), "{example}");
        assert!(stderr.contains(example));
    }
    for private in ["absent", "private-selection-canary", "private-env-canary"] {
        assert!(!value.to_string().contains(private));
        assert!(!stderr.contains(private));
    }
}

#[test]
fn explicit_auth_options_reject_inapplicable_profiles_before_native_setup() {
    let fixture = Fixture::new();
    for (args, field, command) in [
        (
            vec!["auth", "login", "claude-main", "--browser"],
            "--browser",
            "hivectl auth login ",
        ),
        (
            vec!["auth", "login", "api", "--browser"],
            "--browser",
            "hivectl auth login ",
        ),
        (
            vec!["auth", "login", "api", "--reauthenticate"],
            "--reauthenticate",
            "hivectl auth login ",
        ),
        (
            vec!["auth", "repair", "api", "--reauthenticate"],
            "NAME",
            "hivectl auth repair ",
        ),
        (
            vec![
                "auth",
                "repair",
                "claude-main",
                "--reauthenticate",
                "--browser",
            ],
            "--browser",
            "hivectl auth repair ",
        ),
        (
            vec!["auth", "login", "private-selection-canary"],
            "NAME",
            "hivectl auth login ",
        ),
    ] {
        input_error(
            ordering::configured(&fixture, args[0])
                .args(args)
                .env("HIVE_CANARY", "private-env-canary")
                .output()
                .unwrap(),
            field,
            command,
        );
        assert!(!fixture.state.root.exists());
    }
}

#[test]
fn in_place_repair_preserves_format_guard_before_invalid_selection() {
    let fixture = Fixture::new();
    fixture.register(Provider::Codex);
    let index = fixture.state.root.join("index.json");
    let original = std::fs::read(&index).unwrap();
    std::fs::write(&index, "{}").unwrap();
    let destination = fixture.tmp.path().join("replacement");
    for (name, browser, field) in [("api", false, "NAME"), ("claude-main", true, "--browser")] {
        let mut command = ordering::configured(&fixture, "auth");
        command.args(["auth", "repair", name, "--reauthenticate"]);
        if browser {
            command.arg("--browser");
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        ordering::error(output, "unsupported_schema");
        // A replacement skips the old format, but still rejects the selected input.
        input_error(
            command
                .arg("--new-state-dir")
                .arg(&destination)
                .output()
                .unwrap(),
            field,
            "hivectl auth repair ",
        );
        assert!(!destination.exists());
        assert_eq!(std::fs::read(&index).unwrap(), b"{}");
    }
    assert!(!fixture.state.root.join("native-probes").exists());
    assert_ne!(original, b"{}");
}

#[test]
fn valid_login_selection_without_a_terminal_remains_operational() {
    let fixture = Fixture::new();
    for name in ["codex-main", "claude-main"] {
        let output = ordering::configured(&fixture, "auth")
            .args(["auth", "login", name])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        ordering::error(output, "interactive_terminal_required");
    }
    let output = ordering::configured(&fixture, "auth")
        .args(["auth", "login", "api"])
        .env_remove("HIVE_CANARY")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["outcome"], "registered");
    assert_eq!(report["state"], "missing");
    assert!(fixture.state.root.exists());
}
