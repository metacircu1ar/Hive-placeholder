use super::*;
fn cli(f: &Harness, args: &[&str]) -> (Value, std::process::Output) {
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--state-dir")
        .arg(&f.root)
        .args(args)
        .output()
        .unwrap();
    let value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stderr)));
    (value, output)
}
fn spawn(f: &Harness, provider: Provider, body: &str) -> String {
    if provider == Provider::Claude {
        claude::spawn(f, body)
    } else {
        f.spawn(body)
    }
}
provider_matrix!(
    joined_cli_wait_timeout_resume_branch_and_startup,
    human_case
);
fn human_case(provider: Provider) {
    let mut f = Harness::builder(provider).build();
    f.start();
    let worker = spawn(&f, provider, "hold");
    f.wait(&worker, |a| a["state"] == "running");
    let role = if provider == Provider::Claude {
        "claude"
    } else {
        "implementor"
    };
    let operations = json!([
        {"op":"send","target":worker,"body":"reply:delayed","request_id":"one"},
        {"op":"spawn","role":role,"cwd":f.tmp.path(),"body":"reply:startup","request_id":"two"}
    ]);
    let file = f.tmp.path().join("operations.json");
    fs::write(&file, operations.to_string()).unwrap();
    let args = [
        "batch",
        "--operations-file",
        file.to_str().unwrap(),
        "--request-id",
        "joined-round",
        "--join",
        "--deadline-seconds",
        "1",
    ];
    let (accepted, output) = cli(&f, &args);
    assert!(output.status.success(), "{accepted}");
    let join = accepted["join"]["join_id"].as_str().unwrap();
    let (pending, output) = cli(&f, &["wait", "--join", join, "--timeout", "1"]);
    assert!(!output.status.success());
    assert_eq!(pending["wait"], "timeout");
    assert_eq!(cli(&f, &args).0, accepted);
    f.ok(json!({"op":"interrupt","target":worker}));
    let (done, output) = cli(&f, &["wait", "--join", join, "--timeout", "15"]);
    assert!(output.status.success(), "{done}");
    assert_eq!(done["state"], "completed");
    assert_eq!(done["members"].as_array().unwrap().len(), 2);
    for (i, expected) in ["reply:delayed", "reply:startup"].into_iter().enumerate() {
        assert!(
            done["members"][i]["output"]["text"]
                .as_str()
                .unwrap()
                .ends_with(expected)
        );
        assert_eq!(done["members"][i]["outcome"], "completed");
    }
    let branch=f.call(json!({"op":"batch","join":{},"operations":[{"op":"spawn","from_agent":worker,"body":"reply:branched","request_id":"branch"}]}));
    assert!(branch["error"].is_null(), "{branch}");
    let (answer, output) = cli(
        &f,
        &[
            "wait",
            "--join",
            branch["join"]["join_id"].as_str().unwrap(),
            "--timeout",
            "15",
        ],
    );
    assert!(output.status.success(), "{answer}");
    assert!(
        answer["members"][0]["output"]["text"]
            .as_str()
            .unwrap()
            .ends_with("reply:branched")
    );
    f.shutdown();
    f.start();
    assert_eq!(cli(&f, &["read", &format!("join:{join}")]).0, done);
    f.shutdown();
}
provider_matrix!(
    joined_agent_tool_collects_and_wakes_same_conversation,
    agent_case
);
fn agent_case(provider: Provider) {
    let mut f = Harness::builder(provider).build();
    f.start();
    let worker = spawn(&f, provider, "ready");
    f.settled(&worker);
    let input = json!({"request_id":"round","join":{},"operations":[
        {"op":"send","target":worker,"body":"reply:first","request_id":"one"},
        {"op":"send","target":worker,"body":"reply:second","request_id":"two"}]});
    let caller = spawn(&f, provider, &format!("joined:{input}"));
    let result = support::wait::wait_until(
        Duration::from_secs(20),
        Duration::from_millis(25),
        "joined reply",
        || {
            f.mailbox(&caller)["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["kind"] == "join_result" && m["state"] == "completed")
                .cloned()
        },
    );
    assert!(result["body"].as_str().unwrap().contains("reply:first"));
    assert!(result["body"].as_str().unwrap().contains("reply:second"));
    let events = f.read(&caller);
    let starts: Vec<_> = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "turn_started")
        .collect();
    assert_eq!(starts.len(), 2);
    assert_eq!(
        f.mailbox(&caller)["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["kind"] == "join_result")
            .count(),
        1
    );
    f.shutdown();
}
provider_matrix!(
    joined_maximum_escaped_payload_reaches_both_provider_transports,
    size_case
);
fn size_case(provider: Provider) {
    let mut f = Harness::builder(provider)
        .scenario(json!({"echo_input_items": true}))
        .build();
    f.start();
    let worker = spawn(&f, provider, "ready");
    f.settled(&worker);
    // Worst JSON escaping: 4 MiB raw becomes over 24 MiB on the native pipe,
    // including Codex's started/completed user-item echoes through the shard route.
    let ops:Vec<_>=(0..64).map(|i|json!({"op":"send","target":worker,"body":"join-output:control","request_id":format!("m-{i}")})).collect();
    let input = json!({"request_id":"size-round","join":{},"operations":ops});
    let caller = spawn(&f, provider, &format!("joined:{input}"));
    let completed =
        support::wait::try_wait_until(Duration::from_secs(90), Duration::from_millis(50), || {
            // Avoid repeatedly encoding a 24 MiB escaped reply while the
            // provider is still consuming it. Inspect its contents once settled.
            if f.status(&caller)["state"] != "idle" {
                return None;
            }
            let messages = f.mailbox(&caller);
            messages["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["kind"] == "join_result" && m["state"] == "completed")
                .map(|m| {
                    assert_eq!(
                        m["body"]
                            .as_str()
                            .unwrap()
                            .bytes()
                            .filter(|b| *b == 1)
                            .count(),
                        64 * 65536
                    );
                })
        });
    assert!(
        completed.is_some(),
        "maximum joined reply did not settle: caller={}, worker={}",
        f.status(&caller),
        f.status(&worker)
    );
    f.shutdown();
}

#[test]
fn join_input_errors_reach_read_and_wait_without_echoing_missing_ids() {
    let mut f = Harness::new();
    f.start();
    for (args, field) in [
        (vec!["read", "join:private-canary"], "TARGET"),
        (
            vec!["wait", "--join", "private-canary", "--timeout", "1"],
            "--join",
        ),
    ] {
        let (value, output) = cli(&f, &args);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert_eq!(value["error"]["code"], "invalid_value");
        assert_eq!(value["error"]["field"], field);
        assert!(!value.to_string().contains("private-canary"));
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(field));
        assert!(!stderr.contains("private-canary"));
    }
    assert!(
        f.ok(json!({"op":"agents"}))["agents"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    f.shutdown();
}
