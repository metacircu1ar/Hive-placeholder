//! Parser failures must finish before files, controllers or providers are opened.
use super::support;
use serde_json::Value;
use std::path::Path;

fn command(binary: &str, root: &Path) -> std::process::Command {
    let mut command = support::command(binary);
    command
        .arg("--state-dir")
        .arg(root.join("absent-state"))
        .arg("--config")
        .arg(root.join("absent-config.yaml"))
        .arg("--codex-bin")
        .arg(root.join("provider-spy"))
        .arg("--claude-bin")
        .arg(root.join("provider-spy"));
    command
}

fn provider_spy(root: &Path) {
    atomic_fs::Publisher::new(
        (hive_setup_support::names::PUBLICATION_OPTIONS)
            .with_mode(0o700)
            .with_durability(atomic_fs::Durability::AtomicVisible),
    )
    .create_new(
        root.join("provider-spy"),
        b"#!/bin/sh\ntouch \"$0.started\"\nexit 91\n",
    )
    .unwrap();
}

const LEXICAL_CASES: &[(&[&str], &str, &str)] = &[
    (
        &["compact", "--before", "9223372036854775808"],
        "out_of_range",
        "--before",
    ),
    (
        &["compact", "--before", "0", "--keep-per-agent", "0"],
        "out_of_range",
        "--keep-per-agent",
    ),
    (
        &["set-options", "agent", "--option", "model=bad name"],
        "invalid_value",
        "--option",
    ),
    (
        &[
            "spawn",
            "--option",
            "model=bad name",
            "--cwd",
            "/absent",
            "--body-file",
            "PRIVATE",
            "--native-config",
            "PRIVATE",
        ],
        "invalid_value",
        "--option",
    ),
    (
        &["setup", "--codex-effort", "fast"],
        "invalid_effort",
        "--codex-effort",
    ),
    (
        &[
            "spawn",
            "--cwd",
            "/absent",
            "--body-file",
            "PRIVATE",
            "--model",
            "bad name",
        ],
        "invalid_model",
        "--model",
    ),
    (
        &[
            "send",
            "agent",
            "--body-file",
            "PRIVATE",
            "--request-id",
            "PRIVATE\nID",
        ],
        "invalid_request_id",
        "--request-id",
    ),
    (
        &[
            "batch",
            "--operations-file",
            "PRIVATE",
            "--request-id",
            "PRIVATE\nID",
        ],
        "invalid_request_id",
        "--request-id",
    ),
    (
        &["respond", "PRIVATE", "--accept", "--data-file", "PRIVATE"],
        "invalid_uuid",
        "APPROVAL_ID",
    ),
    (
        &["send", "agent", "--body-file", "PRIVATE", "--delay", "8d"],
        "invalid_duration",
        "--delay",
    ),
    (
        &[
            "send",
            "agent",
            "--body-file",
            "PRIVATE",
            "--delay",
            "1s",
            "--mode",
            "redirect",
        ],
        "conflicting_arguments",
        "--mode",
    ),
    (
        &[
            "send",
            "agent",
            "--body-file",
            "PRIVATE",
            "--wait",
            "--mode",
            "steer",
        ],
        "conflicting_arguments",
        "--mode",
    ),
    (
        &["wait", "--message", "message", "--timeout", "601"],
        "out_of_range",
        "--timeout",
    ),
    (
        &["read", "PRIVATE", "--follow", "--limit", "201"],
        "out_of_range",
        "--limit",
    ),
    (
        &["read", "artifact:PRIVATE", "--limit", "3"],
        "out_of_range",
        "--limit",
    ),
    (
        &["read", "join:PRIVATE", "--after", "0"],
        "conflicting_arguments",
        "--after",
    ),
    (
        &["read", "join:PRIVATE", "--limit", "100"],
        "conflicting_arguments",
        "--limit",
    ),
    (
        &["read", "join:PRIVATE", "--follow"],
        "conflicting_arguments",
        "--follow",
    ),
    (
        &["wait", "PRIVATE", "--after", "0"],
        "conflicting_arguments",
        "--after",
    ),
];

#[test]
fn lexical_and_message_errors_precede_config_provider_and_user_file_access() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    provider_spy(root);
    for &(args, code, field) in LEXICAL_CASES {
        let output = command(env!("CARGO_BIN_EXE_hivectl"), root)
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["code"], code, "{args:?}");
        assert_eq!(value["error"]["field"], field, "{args:?}");
        let human = String::from_utf8(output.stderr).unwrap();
        assert!(!human.contains("request ID"));
        assert!(!human.contains("PRIVATE"));
        assert!(!value.to_string().contains("PRIVATE"));
    }
    assert_eq!(std::fs::read_dir(root).unwrap().count(), 1);
    assert!(!root.join("provider-spy.started").exists());
}

#[test]
fn parser_failures_are_structured_private_and_have_no_startup_side_effects() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    // If either provider is invoked, the spy leaves evidence in its own directory.
    provider_spy(root);
    for args in [
        vec!["auth", "login", "codex-main", "--unexpected-PRIVATE"],
        vec!["auth", "login", "claude-main", "--probe-timeout", "0"],
        vec!["spawn", "--body-file", "PRIVATE", "--provider", "wrong"],
        vec!["read", "PRIVATE", "--follow", "--limit", "invalid"],
        vec!["mcp", "--unexpected-PRIVATE"],
        vec!["approvals", "--after", "PRIVATE", "--wait"],
    ] {
        let output = command(env!("CARGO_BIN_EXE_hivectl"), root)
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(report["error"]["code"].is_string());
        let examples = report["error"]["examples"].as_array().unwrap();
        assert!(!examples.is_empty());
        let human = String::from_utf8(output.stderr).unwrap();
        for example in examples {
            assert!(human.contains(example.as_str().unwrap()));
        }
        assert!(
            !String::from_utf8(output.stdout)
                .unwrap()
                .contains("PRIVATE")
        );
        assert!(!human.contains("PRIVATE"));
    }
    assert!(!root.join("provider-spy.started").exists());
    assert!(!root.join("absent-state").exists());
    assert!(!root.join("absent-config.yaml").exists());
    assert_eq!(std::fs::read_dir(root).unwrap().count(), 1);
}

#[test]
fn daemon_help_and_rejected_commands_do_not_start_the_controller() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for args in [
        vec!["--help"],
        vec!["--version"],
        vec!["daemon", "run"],
        vec!["status"],
        vec!["--probe-timeout", "301"],
    ] {
        let output = command(env!("CARGO_BIN_EXE_hive-daemon"), root)
            .args(&args)
            .output()
            .unwrap();
        if matches!(args[0], "--help" | "--version") {
            assert!(output.status.success());
            assert!(output.stderr.is_empty());
            let help = String::from_utf8(output.stdout).unwrap();
            assert!(help.contains("hive-daemon"));
            assert!(!help.contains("Commands:"));
        } else {
            assert_eq!(output.status.code(), Some(2));
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                value["error"]["examples"][0],
                "hive-daemon --config examples/auth.yaml"
            );
        }
    }
    assert_eq!(std::fs::read_dir(root).unwrap().count(), 0);
}

#[test]
fn unused_globals_fail_before_content_defaults_or_daemon_contact() {
    use std::os::unix::net::UnixListener;
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let root = temp.path();
    provider_spy(root);
    std::fs::create_dir(root.join("runtime")).unwrap();
    let listener = UnixListener::bind(hive::runtime::socket_path(root)).unwrap();
    listener.set_nonblocking(true).unwrap();
    for (args, field) in [
        (
            vec![
                "send",
                "agent",
                "--body-file",
                "MISSING",
                "--config",
                "PRIVATE",
            ],
            "--config",
        ),
        (
            vec!["agents", "--codex-bin", "./provider-spy"],
            "--codex-bin",
        ),
        (
            vec![
                "set-options",
                "agent",
                "--option",
                "effort=low",
                "--native-config",
                "MISSING",
                "--config",
                "PRIVATE",
            ],
            "--config",
        ),
        (vec!["agents", "--probe-timeout", "15"], "--probe-timeout"),
        (
            vec![
                "spawn",
                "--cwd",
                "/",
                "--body-file",
                "MISSING",
                "--managed-service",
                "PRIVATE",
            ],
            "--managed-service",
        ),
    ] {
        let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
            .current_dir(root)
            .env_remove("HOME")
            .env("XDG_CONFIG_HOME", "invalid-relative")
            .arg("--state-dir")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["code"], "conflicting_arguments");
        assert_eq!(value["error"]["field"], field);
        assert!(!value.to_string().contains("PRIVATE"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE"));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    assert!(!root.join("index.json").exists());
    assert!(!root.join("provider-spy.started").exists());
}

#[test]
fn selection_and_cursor_errors_precede_payload_reads_and_socket_contact() {
    use std::os::unix::net::UnixListener;
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let root = temp.path();
    std::fs::create_dir(root.join("runtime")).unwrap();
    let listener = UnixListener::bind(hive::runtime::socket_path(root)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let cases = [
        (
            vec!["send", "", "--body-file", "PRIVATE"],
            "TARGET",
            "out_of_range",
        ),
        (
            vec![
                "spawn",
                "private.name",
                "--cwd",
                "/absent",
                "--body-file",
                "PRIVATE",
            ],
            "ROLE",
            "invalid_value",
        ),
        (
            vec![
                "agents",
                "--stalled",
                "1s",
                "--stalled-after",
                r#"{"idle_since":1,"root_id":""}"#,
            ],
            "--stalled-after",
            "out_of_range",
        ),
        (
            vec![
                "agents",
                "--stalled",
                "1s",
                "--stalled-after",
                r#"[1,"PRIVATE"]"#,
            ],
            "--stalled-after",
            "invalid_value",
        ),
        (vec!["wait", "--join", ""], "--join", "out_of_range"),
    ];
    for (args, field, code) in cases {
        let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
            .current_dir(root)
            .env_remove("HOME")
            .env("XDG_CONFIG_HOME", "invalid-relative")
            .arg("--state-dir")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["code"], code);
        assert_eq!(value["error"]["field"], field);
        assert!(!value.to_string().contains("PRIVATE"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE"));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    assert!(!root.join("index.json").exists());
    assert!(!root.join("runtime/runtime.sqlite").exists());
}
