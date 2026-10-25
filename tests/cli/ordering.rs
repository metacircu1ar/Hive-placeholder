//! Config and provider setup occur only for commands that consume them.
use super::{Fixture, Provider, support};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    path::Path,
    process::{Command, Output},
    time::Duration,
};

fn without_config(root: &Path) -> Command {
    let mut command = support::command(env!("CARGO_BIN_EXE_hivectl"));
    command
        .env_remove("HOME")
        .env("XDG_CONFIG_HOME", "invalid-relative")
        .arg("--state-dir")
        .arg(root);
    command
}

#[test]
fn config_check_needs_no_state_home_daemon_or_provider() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config.yaml");
    std::fs::write(&config, "auth_profiles: {}\n").unwrap();
    for fix in [false, true] {
        let mut command = support::command(env!("CARGO_BIN_EXE_hivectl"));
        command
            .env_remove("HOME")
            .env("XDG_STATE_HOME", "invalid-relative")
            .arg("--config")
            .arg(&config)
            .args(["config", "check"]);
        if fix {
            command.arg("--fix");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["valid"], true);
        assert_eq!(value["changed"], false);
        assert_eq!(value["deprecations"], json!([]));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

pub(super) fn error(output: Output, code: &str) -> Value {
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["error"]["code"],
        code,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    value
}

#[test]
fn remote_commands_contact_daemon_without_config_defaults() {
    for (args, op, reply) in [
        (
            vec!["agents"],
            "agents",
            json!({"agents":[],"next_after":null}),
        ),
        (vec!["auth", "groups"], "auth_groups", json!({"groups":[]})),
        (
            vec!["send", "agent", "--body", "work", "--request-id", "retry"],
            "send",
            json!({"message_id":"message"}),
        ),
    ] {
        let temp = tempfile::tempdir_in("/tmp").unwrap();
        std::fs::create_dir(temp.path().join("runtime")).unwrap();
        let listener = UnixListener::bind(hive::runtime::socket_path(temp.path())).unwrap();
        listener.set_nonblocking(true).unwrap();
        let worker = std::thread::spawn(move || {
            let mut stream = support::wait::wait_until(
                Duration::from_secs(5),
                Duration::from_millis(5),
                "remote command",
                || match listener.accept() {
                    Ok((stream, _)) => Some(stream),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => None,
                    Err(error) => panic!("{error}"),
                },
            );
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["args"]["op"], op);
            if op == "send" {
                assert_eq!(request["id"], "retry");
            }
            writeln!(stream, "{reply}").unwrap();
        });
        let output = without_config(temp.path()).args(args).output().unwrap();
        worker.join().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

#[test]
fn state_only_auth_uses_existing_handlers_with_missing_config() {
    let fixture = Fixture::new();
    let binding = fixture.register(Provider::Codex);
    std::fs::remove_file(&fixture.config).unwrap();
    let output = without_config(&fixture.state.root)
        .args(["auth", "generations", "codex-main"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["generations"].as_array().unwrap().len(), 1);
    let rejected = without_config(&fixture.state.root)
        .args([
            "auth",
            "prune",
            "codex-main",
            "--generation",
            &binding.id.to_string(),
        ])
        .output()
        .unwrap();
    let report = error(rejected, "invalid_request");
    assert!(
        report["error"]["message"]
            .as_str()
            .unwrap()
            .contains("inactive retired")
    );
    assert!(binding.directory.exists());
    let destination = fixture.tmp.path().join("export.jsonl");
    error(
        without_config(&fixture.state.root)
            .args(["auth", "history-export", "--output"])
            .arg(&destination)
            .output()
            .unwrap(),
        "invalid_request",
    );
    assert!(!destination.exists());
    // A corrupt state must still reach the format/ownership guard, not be hidden.
    std::fs::write(fixture.state.root.join("index.json"), "{}").unwrap();
    error(
        without_config(&fixture.state.root)
            .args(["auth", "generations", "codex-main"])
            .output()
            .unwrap(),
        "unsupported_schema",
    );
}

pub(super) fn configured(fixture: &Fixture, command_name: &str) -> Command {
    let mut command = without_config(&fixture.state.root);
    command.arg("--config").arg(&fixture.config);
    if command_name != "list" {
        command
            .arg("--claude-bin")
            .arg(fixture.tmp.path().join("absent-claude"));
        if command_name != "verify-steering" {
            command
                .arg("--codex-bin")
                .arg(fixture.tmp.path().join("absent-codex"));
        }
    }
    command
}

#[test]
fn profile_and_canary_selection_precede_provider_setup_and_state_creation() {
    let fixture = Fixture::new();
    let output = configured(&fixture, "list")
        .args(["auth", "list"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["profiles"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    for args in [
        vec!["auth", "status", "absent"],
        vec!["auth", "login", "absent"],
        vec!["auth", "recover", "absent"],
        vec!["auth", "repair", "absent", "--reauthenticate"],
        vec!["models", "--auth", "absent"],
        vec![
            "verify-steering",
            "--auth",
            "absent",
            "--model",
            "sonnet",
            "--effort",
            "low",
        ],
    ] {
        let field = if args[0] == "auth" { "NAME" } else { "--auth" };
        let key = if args[0] == "auth" {
            format!("hivectl auth {} ", args[1])
        } else {
            format!("hivectl {} ", args[0])
        };
        super::auth_selection::input_error(
            configured(&fixture, args[0]).args(args).output().unwrap(),
            field,
            &key,
        );
    }
    for name in ["codex-main", "api"] {
        super::auth_selection::input_error(
            configured(&fixture, "verify-steering")
                .args([
                    "verify-steering",
                    "--auth",
                    name,
                    "--model",
                    "sonnet",
                    "--effort",
                    "low",
                ])
                .output()
                .unwrap(),
            "--auth",
            "hivectl verify-steering ",
        );
    }
    assert!(!fixture.state.root.exists());
}

#[test]
fn auth_groups_keeps_its_absolute_state_path_check_without_config() {
    let temp = tempfile::tempdir().unwrap();
    let result = error(
        without_config(Path::new("relative"))
            .current_dir(temp.path())
            .args(["auth", "groups"])
            .output()
            .unwrap(),
        "invalid_path",
    );
    assert_eq!(
        result["error"]["message"],
        "State directory must be absolute."
    );
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}
