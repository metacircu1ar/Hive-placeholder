use super::*;
use std::os::unix::{ffi::OsStringExt, fs::MetadataExt};

fn command(f: &Harness, binary: &str, root: &std::path::Path) -> std::process::Command {
    let mut command = support::command(binary);
    command
        .arg("--config")
        .arg(&f.config)
        .arg("--state-dir")
        .arg(root)
        .arg("--codex-bin")
        .arg(&f.binary)
        .arg("--claude-bin")
        .arg(f.tmp.path().join("claude"));
    command
}
fn records(f: &Harness) -> Vec<Value> {
    let path = f.root.join("runtime/logs/hive-daemon.jsonl");
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    let bytes = fs::read(path).unwrap();
    assert!(bytes.ends_with(b"\n"));
    bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect()
}

#[test]
fn shutdown_logs_distinguish_control_from_each_unix_signal() {
    for signal in [
        None,
        Some(libc::SIGINT),
        Some(libc::SIGTERM),
        Some(libc::SIGHUP),
    ] {
        let mut f = Harness::new();
        f.start();
        if let Some(signal) = signal {
            let child = f.daemon.as_mut().unwrap();
            let pid = i32::try_from(child.id()).unwrap();
            // SAFETY: The owned unreaped child cannot have its PID reused.
            assert_eq!(unsafe { libc::kill(pid, signal) }, 0);
            let status = support::wait::wait_until(
                support::wait::SHUTDOWN_TIMEOUT,
                support::wait::POLL_INTERVAL,
                "signal-triggered daemon shutdown",
                || child.try_wait().unwrap(),
            );
            assert!(status.success());
            f.daemon = None;
        } else {
            f.shutdown();
        }
        let rows = records(&f);
        let triggers: Vec<_> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                row["event"] == "daemon.signal" || row["event"] == "daemon.shutdown_requested"
            })
            .collect();
        assert_eq!(triggers.len(), 1, "{rows:?}");
        let (trigger, row) = triggers[0];
        if let Some(signal) = signal {
            assert_eq!(row["event"], "daemon.signal");
            assert_eq!(row["signal"], signal);
        } else {
            assert_eq!(row["event"], "daemon.shutdown_requested");
        }
        let ready = rows
            .iter()
            .position(|row| row["event"] == "daemon.ready")
            .unwrap();
        let stopped = rows
            .iter()
            .position(|row| row["event"] == "daemon.stopped")
            .unwrap();
        assert!(ready < trigger && trigger < stopped);
        assert_eq!(rows[stopped]["success"], true);
    }
}

#[test]
fn daemon_logs_survive_restart_and_reject_a_competing_owner() {
    let mut f = Harness::new();
    f.start();
    let id = f.spawn("sk-secret-body");
    f.settled(&id);
    f.shutdown();
    let before = fs::read(f.root.join("runtime/logs/hive-daemon.jsonl")).unwrap();
    f.start();
    let contender = command(&f, env!("CARGO_BIN_EXE_hivectl"), &f.root)
        .args(["daemon", "run"])
        .output()
        .unwrap();
    assert!(!contender.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&contender.stdout).unwrap()["error"]["code"],
        "daemon_busy"
    );
    f.shutdown();
    let after = fs::read(f.root.join("runtime/logs/hive-daemon.jsonl")).unwrap();
    assert!(after.starts_with(&before));
    assert!(!String::from_utf8_lossy(&after).contains("sk-secret"));
    let rows = records(&f);
    for event in [
        "daemon.starting",
        "daemon.lock_acquired",
        "daemon.socket_bound",
        "daemon.ready",
        "daemon.stopped",
    ] {
        assert_eq!(
            rows.iter().filter(|r| r["event"] == event).count(),
            2,
            "{event}: {rows:?}"
        );
    }
    assert!(
        rows.iter()
            .filter(|r| r["event"] == "daemon.stopped")
            .all(|r| r["success"] == true)
    );
}

#[test]
fn startup_failure_after_logger_install_is_sanitized_and_drained() {
    let f = Harness::new();
    let dir = f.root.join("runtime");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(dir.join("control.sock"), "sk-secret-blocked-socket").unwrap();
    let output = command(&f, env!("CARGO_BIN_EXE_hivectl"), &f.root)
        .args(["daemon", "run"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let rows = records(&f);
    assert!(
        rows.iter()
            .any(|r| r["event"] == "daemon.failed" && r["code"] == "unsafe_state")
    );
    assert!(
        rows.iter()
            .any(|r| r["event"] == "daemon.stopped" && r["success"] == false)
    );
    assert!(!rows.iter().any(|r| r["event"] == "daemon.ready"));
    assert!(!serde_json::to_string(&rows).unwrap().contains("sk-secret"));
}

#[test]
fn invalid_log_level_cannot_create_state_for_any_daemon_entrypoint() {
    let f = Harness::new();
    for level in [
        std::ffi::OsString::from("sk-secret-invalid-level"),
        std::ffi::OsString::from_vec(vec![255]),
    ] {
        for (binary, args) in [
            (env!("CARGO_BIN_EXE_hivectl"), vec!["daemon", "start"]),
            (env!("CARGO_BIN_EXE_hivectl"), vec!["daemon", "run"]),
            (env!("CARGO_BIN_EXE_hive-daemon"), vec![]),
        ] {
            let root = f.tmp.path().join("not-created");
            let output = command(&f, binary, &root)
                .args(args)
                .env("HIVE_LOG", &level)
                .output()
                .unwrap();
            assert!(!output.status.success());
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(value["error"]["code"], "config_invalid");
            assert!(!root.exists());
            assert!(!String::from_utf8_lossy(&output.stdout).contains("sk-secret"));
        }
    }
    let mut f = f;
    f.start();
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--config")
        .arg(&f.config)
        .arg("--state-dir")
        .arg(&f.root)
        .args(["daemon", "status"])
        .env("HIVE_LOG", "invalid")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cli.log_level_fallback"));
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--state-dir")
        .arg(&f.root)
        .args(["daemon", "stop"])
        .env("HIVE_LOG", "invalid")
        .output()
        .unwrap();
    assert!(output.status.success());
    let status = support::wait::wait_until(
        support::wait::SHUTDOWN_TIMEOUT,
        support::wait::POLL_INTERVAL,
        "daemon stopped with invalid CLI logging override",
        || f.daemon.as_mut().unwrap().try_wait().unwrap(),
    );
    assert!(status.success());
    f.daemon = None;
}

#[test]
fn startup_turn_shutdown_produce_current_logs_without_sidecars() {
    let mut f = Harness::new();
    f.start();
    let agent = f.spawn("complete");
    support::wait::wait_until(
        support::wait::TEST_TIMEOUT,
        support::wait::POLL_INTERVAL,
        "completed mock turn",
        || {
            let status = f.ok(json!({"op":"status", "target":agent}));
            (status["agent"]["task_outcome"] == "completed").then_some(())
        },
    );
    f.shutdown();
    let rows = records(&f);
    assert!(rows.iter().any(|row| row["event"] == "daemon.starting"));
    assert!(rows.iter().any(|row| row["event"] == "daemon.stopped"));
    assert!(!rows.iter().any(|row| {
        ["writer.batch", "runtime.timing", "state.sync"].contains(&row["event"].as_str().unwrap())
    }));
    for entry in fs::read_dir(f.root.join("runtime")).unwrap() {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        assert!(!name.starts_with("diagnostics-") && !name.starts_with("profile-"));
    }
    assert!(f.reports().iter().all(|report| report["confirmed"] == true));
}
