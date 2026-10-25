use super::*;
use std::process::Stdio;

#[test]
fn managed_start_failure_reports_a_sanitized_cause_before_logger_installation() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--state-dir")
        .arg(&state)
        .arg("--config")
        .arg(temp.path().join("config.yaml"))
        .arg("--managed-service")
        .arg(temp.path().join("secret-service-path"))
        .args(["daemon", "run"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value["error"]["code"].is_string());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let diagnostic = stderr
        .lines()
        .find(|line| line.contains("cli.service_failed"))
        .unwrap();
    assert!(diagnostic.contains("service_prepare"), "{diagnostic}");
    assert!(!diagnostic.contains("secret-service-path"), "{diagnostic}");
    assert!(!state.exists());
}

#[test]
fn logs_follow_prints_complete_records_without_a_daemon_and_exits_on_interrupt() {
    use std::{
        io::Write,
        os::unix::{fs::OpenOptionsExt, process::ExitStatusExt},
    };
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    hive::state::directory(&root).unwrap();
    hive::state::directory(&root.join("runtime")).unwrap();
    hive::state::directory(&root.join("runtime/logs")).unwrap();
    let path = root.join("runtime/logs/hive-daemon.jsonl");
    let mut writer = fs::OpenOptions::new()
        .create_new(true)
        .append(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    writer
        .write_all(
            b"{\"event\":\"agent.spawned\",\"level\":\"info\",\"agent_id\":\"a\"}\n{\"event\":",
        )
        .unwrap();
    let stdout = temp.path().join("stdout");
    let stderr = temp.path().join("stderr");
    let mut child = support::ChildGuard::new(
        support::command(env!("CARGO_BIN_EXE_hivectl"))
            .arg("--state-dir")
            .arg(&root)
            .args(["logs", "--follow", "--agent", "a"])
            .stdout(fs::File::create(&stdout).unwrap())
            .stderr(fs::File::create(&stderr).unwrap())
            .spawn()
            .unwrap(),
    );
    wait_until(
        Duration::from_secs(5),
        Duration::from_millis(10),
        "initial log tail",
        || (fs::read_to_string(&stdout).unwrap().lines().count() == 1).then_some(()),
    );
    writer
        .write_all(b"\"agent.completed\",\"level\":\"info\",\"agent_id\":\"a\"}\n")
        .unwrap();
    wait_until(
        Duration::from_secs(5),
        Duration::from_millis(10),
        "appended log record",
        || (fs::read_to_string(&stdout).unwrap().lines().count() == 2).then_some(()),
    );
    rustix::process::kill_process(
        rustix::process::Pid::from_child(&child),
        rustix::process::Signal::INT,
    )
    .unwrap();
    let status = wait_until(
        Duration::from_secs(5),
        Duration::from_millis(10),
        "interrupted log viewer",
        || child.try_wait().unwrap(),
    );
    assert_eq!(status.signal(), Some(libc::SIGINT));
    let rows: Vec<Value> = fs::read_to_string(stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["event"], "agent.completed");
    assert!(
        fs::read_to_string(stderr)
            .unwrap()
            .contains("incomplete trailing record")
    );
    assert!(!root.join("runtime/runtime.sqlite").exists());
    assert!(!root.join("runtime/logs/.hive-daemon.jsonl.lock").exists());
}

#[test]
fn logs_missing_state_and_invalid_options_never_create_state() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("missing-state");
    for options in [
        vec!["logs"],
        vec!["logs", "--tail", "0"],
        vec!["logs", "--level", "secret-bad-level"],
    ] {
        let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
            .arg("--state-dir")
            .arg(&root)
            .args(options)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!root.exists());
    }
}

#[test]
fn standalone_cleanup_failure_is_visible_without_replacing_probe_timeout() {
    let fixture = Fixture::new();
    let binding = fixture.register(Provider::Codex);
    let ready = fixture.tmp.path().join("probe-ready");
    let stop = fixture.tmp.path().join("probe-stop");
    let log = fixture.tmp.path().join("stderr.log");
    fixture.scenario(json!({"hang":true,"ready":ready,"stop_file":stop}));
    let _release = ProbeRelease::new(&stop, &ready);
    let mut command = fixture.command();
    if let Some(binary) = std::env::var_os("HIVE_LOGGING_TEST_BINARY") {
        let binary = PathBuf::from(binary);
        assert_eq!(binary.parent().unwrap().file_name().unwrap(), "release");
        let args: Vec<_> = command.get_args().map(std::ffi::OsStr::to_owned).collect();
        command = support::command(binary);
        command.args(args);
    }
    let mut child = support::ChildGuard::new(
        command
            .args(["status", "codex-main"])
            .stdout(Stdio::piped())
            .stderr(fs::File::create(&log).unwrap())
            .spawn()
            .unwrap(),
    );
    wait_until(
        PROBE_READY_TIMEOUT,
        PROBE_POLL_INTERVAL,
        "probe ready",
        || ready.exists().then_some(()),
    );
    let pid: u32 = fs::read_to_string(&ready).unwrap().parse().unwrap();
    let entries: Vec<_> = fs::read_dir(binding.directory.join("executions"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    assert_eq!(entries.len(), 1);
    // Deliberately corrupt this test-owned active receipt after authorization.
    // Cleanup must still reap the process and report its secondary failure.
    fs::write(&entries[0], b"{\"secret-canary-receipt-body\":").unwrap();
    wait_until(
        Duration::from_secs(12),
        PROBE_POLL_INTERVAL,
        "CLI cleanup",
        || child.try_wait().unwrap(),
    );
    let output = child.wait_with_stdout().unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["diagnostic"]["code"], "timeout", "{value}");
    let stderr = fs::read_to_string(&log).unwrap();
    assert!(
        stderr.contains("process.cleanup_receipt_failed"),
        "{stderr}"
    );
    assert!(stderr.contains("retain_evidence"), "{stderr}");
    assert!(!stderr.contains("secret-canary-receipt-body"));
    assert!(hive::state::ProcessIdentity::read(pid).unwrap().is_none());
    assert!(!fixture.state.root.join("runtime/logs").exists());
}

#[test]
fn daemon_rejects_invalid_configuration_before_creating_the_state_parent() {
    for (contents, level, expected) in [
        (None, "info", "config_unavailable"),
        (Some("auth_profiles: ["), "info", "config_invalid"),
        (
            Some("auth_profiles: {}\n"),
            "invalid-level",
            "config_invalid",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("not-created");
        let config = temp.path().join("config.yaml");
        if let Some(contents) = contents {
            fs::write(&config, contents).unwrap();
        }
        let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
            .arg("--config")
            .arg(&config)
            .arg("--state-dir")
            .arg(parent.join("state"))
            .args(["daemon", "run"])
            .env("HIVE_LOG", level)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply["error"]["code"], expected, "{reply}");
        assert!(
            !parent.exists(),
            "Invalid startup created state admission files"
        );
    }
}
