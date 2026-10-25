use super::*;

fn cli(f: &Harness, args: &[&str]) -> (Value, bool) {
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--state-dir")
        .arg(&f.root)
        .args(args)
        .output()
        .unwrap();
    (
        serde_json::from_slice(&output.stdout).unwrap(),
        output.status.success(),
    )
}
fn inputs(home: &std::path::Path) -> Vec<Value> {
    fs::read_to_string(home.join("inputs.jsonl"))
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}
provider_matrix!(
    delayed_cli_wait_restart_and_self_reminders_on_both_providers,
    delayed_cli_wait_restart_and_self_reminders_on_both_providers_case
);

fn delayed_cli_wait_restart_and_self_reminders_on_both_providers_case(provider: Provider) {
    let is_claude = provider == Provider::Claude;
    let mut f = if is_claude {
        claude::fixture()
    } else {
        Harness::new()
    };
    f.start();
    let role = if is_claude { "claude" } else { "implementor" };
    let home = if is_claude {
        claude::home(&f)
    } else {
        f.provider_home.clone()
    };
    let spawn = f.call(json!({"op":"spawn","role":role,"cwd":f.tmp.path(),"body":"reply:ready"}));
    let id = spawn["agent_id"].as_str().unwrap().to_owned();
    lifecycle::wait_completed_input(&f, &id, spawn["message_id"].as_str().unwrap());
    f.settled(&id);
    f.shutdown();
    f.start();
    let original = f.wait(&id, |a| a["state"] == "stopped");
    assert_eq!(original["paused"], false);
    let body = f.tmp.path().join("message.txt");
    fs::write(&body, "reply:delayed").unwrap();
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let args = [
        "send",
        &id,
        "--body-file",
        body.to_str().unwrap(),
        "--delay",
        "3s",
        "--request-id",
        "timer",
    ];
    let (sent, ok) = cli(&f, &args);
    assert!(ok, "{sent}");
    let message = sent["message_id"].as_str().unwrap();
    assert_eq!(sent["delivery_state"], "scheduled");
    assert_eq!(inputs(&home).len(), 1);
    assert_eq!(
        f.call(json!({"op":"status","target":id}))["agent"]["state"],
        "stopped"
    );
    let (timed, ok) = cli(&f, &["wait", "--message", message, "--timeout", "1"]);
    assert!(!ok);
    assert_eq!(timed["outcome"], "timeout", "{timed}");
    assert_eq!(timed["result"]["delivery_state"], "scheduled");
    assert_eq!(inputs(&home).len(), 1);
    f.shutdown();
    f.start();
    assert_eq!(cli(&f, &args).0, sent);
    let (answered, ok) = cli(&f, &["wait", "--message", message, "--timeout", "15"]);
    assert!(ok, "{answered}");
    assert_eq!(answered["outcome"], "replied");
    assert_eq!(answered["result"]["thread_id"], original["thread_id"]);
    let native = inputs(&home);
    assert_eq!(native.len(), 2);
    assert_eq!(native[1]["body"], "reply:delayed");
    assert!(native[1]["at"].as_f64().unwrap() >= sent["not_before"].as_f64().unwrap());
    assert!(native[1]["at"].as_f64().unwrap() >= started + 3.0);
    f.wait(&id, |a| a["state"] == "idle");
    assert_self_reminder(&f, &id, &home, &original);
    f.ok(json!({"op":"terminate","target":id}));
    f.shutdown();
}

fn assert_self_reminder(f: &Harness, id: &str, home: &std::path::Path, original: &Value) {
    let self_request = f.call(json!({"op":"send","target":id,"body":"delay-self"}));
    lifecycle::wait_completed_input(f, id, self_request["message_id"].as_str().unwrap());
    let messages = f.mailbox(id);
    let reminder = messages["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["body"] == "reply:reminder")
        .unwrap();
    let reminder_id = reminder["id"].as_str().unwrap().to_owned();
    assert!(reminder["not_before"].is_u64());
    lifecycle::wait_completed_input(f, id, &reminder_id);
    let after = f.wait(id, |a| a["state"] == "idle");
    assert_eq!(after["thread_id"], original["thread_id"]);
    let native = inputs(home);
    assert_eq!(native.len(), 4);
    assert!(native[3]["at"].as_f64().unwrap() >= reminder["not_before"].as_f64().unwrap());
}
