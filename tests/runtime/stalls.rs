use super::*;

#[test]
fn stalled_cli_reports_quiet_mock_then_excludes_queued_work() {
    let mut f = Harness::new();
    f.start();
    let spawned =
        f.call(json!({"op":"spawn","role":"implementor","cwd":f.tmp.path(),"body":"reply:ready"}));
    let agent = spawned["agent_id"].as_str().unwrap().to_owned();
    lifecycle::wait_completed_input(&f, &agent, spawned["message_id"].as_str().unwrap());
    let idle = f.settled(&agent);
    assert!(idle["idle_since"].is_u64());
    let cli = |extra: &[&str]| {
        let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
            .arg("--state-dir")
            .arg(&f.root)
            .args(["agents", "--stalled", "1s"])
            .args(extra)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let page = support::wait::wait_until(
        Duration::from_secs(5),
        Duration::from_millis(30),
        "stalled group",
        || {
            let page = cli(&[]);
            (!page["groups"].as_array().unwrap().is_empty()).then_some(page)
        },
    );
    assert_eq!(page["groups"][0]["root_id"], agent);
    assert_eq!(page["groups"][0]["open_count"], 1);
    f.ok(json!({"op":"stop","target":agent}));
    f.wait(&agent, |a| a["state"] == "stopped");
    let quiet = cli(&[]);
    assert_eq!(quiet["groups"][0]["any_paused"], true);
    f.ok(json!({"op":"send","target":agent,"body":"reply:queued behind pause"}));
    assert!(cli(&[])["groups"].as_array().unwrap().is_empty());
    f.ok(json!({"op":"terminate","target":agent}));
    f.shutdown();
}
