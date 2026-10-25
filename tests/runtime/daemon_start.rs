use super::*;
use std::path::Path;

fn start(f: &Harness, config: &Path, codex: &Path, claude: &Path) -> std::process::Output {
    support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--state-dir")
        .arg(&f.root)
        .arg("--config")
        .arg(config)
        .arg("--codex-bin")
        .arg(codex)
        .arg("--claude-bin")
        .arg(claude)
        .args(["daemon", "start"])
        .output()
        .unwrap()
}

fn conflict(f: &Harness, config: &Path, codex: &Path, claude: &Path, field: &str, pid: &Value) {
    let output = start(f, config, codex, claude);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "conflicting_arguments");
    assert_eq!(value["error"]["field"], field);
    assert!(String::from_utf8(output.stderr).unwrap().contains(field));
    assert!(!value.to_string().contains("private-canary"));
    assert_eq!(&f.ok(json!({"op":"ping"}))["pid"], pid);
    assert!(!f.root.join("runtime/daemon.log").exists());
}

#[test]
fn daemon_start_matches_running_inputs_and_never_restarts_on_conflict() {
    let mut f = Harness::new();
    let claude = f.tmp.path().join("claude"); // deliberately absent unused provider
    f.start();
    let pid = f.ok(json!({"op":"ping"}))["pid"].clone();
    let output = start(&f, &f.config, &f.binary, &claude);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["pid"],
        pid
    );
    let link = f.tmp.path().join("linked.yaml");
    std::os::unix::fs::symlink(&f.config, &link).unwrap();
    assert!(start(&f, &link, &f.binary, &claude).status.success());
    let copied = f.tmp.path().join("private-canary.yaml");
    fs::copy(&f.config, &copied).unwrap();
    conflict(&f, &copied, &f.binary, &claude, "--config", &pid);
    let other = f.tmp.path().join("private-canary-provider");
    conflict(&f, &f.config, &other, &claude, "--codex-bin", &pid);
    conflict(&f, &f.config, &f.binary, &other, "--claude-bin", &pid);
    // Config identity is exact bytes, even when parsed settings are unchanged.
    fs::write(
        &f.config,
        format!("{}\n# changed\n", fs::read_to_string(&f.config).unwrap()),
    )
    .unwrap();
    conflict(&f, &f.config, &f.binary, &claude, "--config", &pid);
    f.shutdown();
}

#[test]
fn daemon_start_initializes_a_fresh_root_before_creating_its_log() {
    let mut f = Harness::new();
    f.root = f.tmp.path().join("fresh-state");
    let claude = f.tmp.path().join("unused-claude");
    let output = start(&f, &f.config, &f.binary, &claude);
    let marker = fs::read(f.root.join("format"));
    // Always request cleanup before assertions: daemon start owns its detached
    // child, while this fixture owns the fresh isolated state directory.
    let shutdown = runtime::client(&f.root, "fresh-root-shutdown", json!({"op":"shutdown"}));
    let stopped = support::wait::try_wait_until(
        support::wait::SHUTDOWN_TIMEOUT,
        support::wait::POLL_INTERVAL,
        || (!f.root.join("runtime/control.sock").exists()).then_some(()),
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(marker.unwrap(), b"{\"format\":1}\n");
    assert!(shutdown.is_ok(), "{shutdown:?}");
    assert!(stopped.is_some(), "fresh daemon did not remove its socket");
    State::new(f.root.clone()).unwrap().check_format().unwrap();
}

#[test]
fn explicit_restart_waits_for_cleanup_and_preserves_saved_agents() {
    let mut f = Harness::new();
    f.start();
    let agent = f.spawn("restart fixture");
    f.wait(&agent, |s| s["state"] == "idle");
    let first = f.ok(json!({"op":"ping"}));
    let mut old = f.daemon.take().unwrap();
    // Reap our foreground child while the CLI waits for ownership to clear.
    let cleanup_root = f.root.clone();
    let reaper = support::process::ThreadGuard::new(
        std::thread::spawn(move || {
            let status = support::wait::wait_until(
                support::wait::SHUTDOWN_TIMEOUT,
                support::wait::POLL_INTERVAL,
                "restarted daemon exit",
                || old.try_wait().unwrap(),
            );
            assert!(status.success());
        }),
        move || {
            let _ = runtime::client(&cleanup_root, "reaper-cleanup", json!({"op":"shutdown"}));
        },
    );
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--state-dir")
        .arg(&f.root)
        .arg("--config")
        .arg(&f.config)
        .arg("--codex-bin")
        .arg(&f.binary)
        .arg("--claude-bin")
        .arg(f.tmp.path().join("claude"))
        .args(["daemon", "restart"])
        .output()
        .unwrap();
    let observed = runtime::client(&f.root, "after-restart", json!({"op":"ping"}));
    let agent_status = runtime::client(
        &f.root,
        "after-restart-agent",
        json!({"op":"status","target":agent}),
    );
    // Request detached-child cleanup before asserting, including on CLI failure.
    let shutdown = runtime::client(&f.root, "restart-cleanup", json!({"op":"shutdown"}));
    let stopped = support::wait::try_wait_until(
        support::wait::SHUTDOWN_TIMEOUT,
        support::wait::POLL_INTERVAL,
        || {
            (!f.root.join("runtime/owner.json").exists()
                && !f.root.join("runtime/control.sock").exists())
            .then_some(())
        },
    );
    reaper.join().unwrap();
    assert!(output.status.success(), "{output:?}");
    let second = observed.unwrap();
    assert_ne!(first["pid"], second["pid"]);
    assert_eq!(first["binary_sha256"], second["binary_sha256"]);
    assert_eq!(second["protocols"], json!({"minimum":1,"maximum":1}));
    assert_eq!(second["agent_catalog"], 1);
    assert!(agent_status.unwrap().get("error").is_none());
    assert!(shutdown.is_ok());
    assert!(stopped.is_some());
}
