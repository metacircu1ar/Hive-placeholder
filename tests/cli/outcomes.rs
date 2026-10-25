use super::support;
use serde_json::Value;
use std::{
    io::{Read, Write},
    process::Stdio,
    time::Duration,
};

#[test]
fn local_runtime_failures_have_safe_json_human_diagnostics_and_exit_one() {
    let temp = tempfile::tempdir().unwrap();
    for follow in [false, true] {
        let mut command = support::command(env!("CARGO_BIN_EXE_hivectl"));
        command
            .env_remove("HOME")
            .env_remove("XDG_STATE_HOME")
            .args(["read", "private-agent-canary"]);
        if follow {
            command.arg("--follow");
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["code"], "invalid_path");
        assert!(String::from_utf8_lossy(&output.stderr).contains("HOME"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("private-agent-canary"));
    }
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--config")
        .arg(temp.path().join("PRIVATE-missing"))
        .arg("--state-dir")
        .arg(temp.path().join("state"))
        .args(["auth", "list"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "config_unavailable");
    assert!(String::from_utf8_lossy(&output.stderr).contains("config_unavailable"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE"));
    assert!(!temp.path().join("state").exists());
}

#[test]
fn mcp_setup_failure_never_emits_a_non_protocol_stdout_record() {
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .env_remove("HOME")
        .env_remove("XDG_STATE_HOME")
        .arg("mcp")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid_path"));
}

#[test]
fn mcp_transport_failure_exits_one_without_a_trailing_cli_response() {
    let temp = tempfile::tempdir().unwrap();
    let mut child = support::ChildGuard::new(
        support::command(env!("CARGO_BIN_EXE_hivectl"))
            .arg("--state-dir")
            .arg(temp.path().join("state"))
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&vec![b'x'; 1024 * 1024 + 1])
        .unwrap();
    let status = support::wait::wait_until(
        Duration::from_secs(5),
        Duration::from_millis(5),
        "MCP transport failure",
        || child.try_wait().unwrap(),
    );
    assert_eq!(status.code(), Some(1));
    let mut stdout = String::new();
    let mut stderr = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(stdout.is_empty());
    assert!(stderr.contains("output_limit"));
    assert!(!temp.path().join("state").exists());
}
