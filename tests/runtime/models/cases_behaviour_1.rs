//! Behaviour regressions.
use super::*;

#[test]
fn standalone_catalog_discovery_does_not_require_a_daemon_lock() {
    let f = Harness::new();
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--config")
        .arg(&f.config)
        .arg("--state-dir")
        .arg(&f.root)
        .arg("--codex-bin")
        .arg(&f.binary)
        .args(["models", "--auth", "main"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        result["models"]["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|model| model["model"] == "gpt-5.6-luna")
    );
}

#[test]
fn codex_session_change_drains_a_process_that_retains_the_unsubscribed_thread() {
    let mut f = Harness::builder(Provider::Codex)
        .scenario(json!({
            "provider_version":"0.163.0",
            "retain_loaded_after_unsubscribe":true
        }))
        .build();
    f.start();
    let id = f.spawn("hold");
    let original = f.wait(&id, |a| a["state"] == "running");
    let first: Value = serde_json::from_str(
        fs::read_to_string(f.provider_home.join("thread-selections.jsonl"))
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let old_pid = u32::try_from(first["pid"].as_u64().unwrap()).unwrap();
    let old_process = hive::state::ProcessIdentity::read(old_pid)
        .unwrap()
        .unwrap();

    f.ok(json!({"op":"set_options","target":id,
        "options":{"codex.auto_compact_token_limit":200_000}}));
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    // The resident source retains its startup scenario. The destination reads
    // this update, allowing ordinary cleanup after the fencing assertion.
    f.update_scenario(Provider::Codex, |scenario| {
        scenario["retain_loaded_after_unsubscribe"] = json!(false);
    });
    f.ok(json!({"op":"interrupt","target":id}));
    let reconfigured = f.wait_timeout(&id, Duration::from_secs(20), |a| {
        a["state"] == "running" && a["execution_id"] != original["execution_id"]
    });
    assert_eq!(reconfigured["thread_id"], original["thread_id"]);
    assert!(old_process.absent().unwrap());
    let starts: Vec<Value> = fs::read_to_string(f.provider_home.join("thread-selections.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(starts.len(), 2, "{starts:?}");
    assert_ne!(starts[0]["pid"], starts[1]["pid"]);
    assert_eq!(starts[1]["method"], "thread/resume");
    assert_eq!(
        starts[1]["config"]["model_auto_compact_token_limit"],
        200_000
    );
    f.shutdown();
}
