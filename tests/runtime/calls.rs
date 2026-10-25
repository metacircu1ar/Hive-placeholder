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

#[test]
fn external_claude_call_returns_the_complete_split_native_message() {
    let mut f = claude::fixture();
    f.start();
    let id = claude::spawn(&f, "ready");
    f.settled(&id);
    let input = f.tmp.path().join("split.txt");
    fs::write(&input, "reply:split").unwrap();
    let (reply, output) = cli(
        &f,
        &[
            "send",
            &id,
            "--body-file",
            input.to_str().unwrap(),
            "--request-id",
            "split-message",
            "--wait",
            "--timeout",
            "10",
        ],
    );
    assert!(output.status.success(), "{reply}");
    assert_eq!(reply["outcome"], "replied");
    assert_eq!(
        reply["result"]["output"]["text"],
        "first block\nsecond block"
    );
    assert_eq!(reply["result"]["output"]["aggregation"], "native_message");
    let events = f.call(json!({"op":"read","target":id,
        "after":reply["result"]["events_after"],"limit":200}));
    let text: Vec<_> = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["type"] == "assistant_message")
        .map(|event| event["data"]["text"].clone())
        .collect();
    assert_eq!(
        text,
        vec![
            json!("first block"),
            json!("second block"),
            json!("first block\nsecond block")
        ]
    );
    f.shutdown();
}

#[test]
fn external_claude_split_reply_publishes_one_shared_aggregate_artifact() {
    let mut f = claude::fixture();
    f.start();
    let id = claude::spawn(&f, "ready");
    f.settled(&id);
    let sent = f.call(json!({"op":"send","target":id,"body":"reply:split-large"}));
    let (reply, output) = cli(
        &f,
        &[
            "wait",
            "--message",
            sent["message_id"].as_str().unwrap(),
            "--timeout",
            "10",
        ],
    );
    assert!(output.status.success(), "{reply}");
    let artifact = &reply["result"]["output"]["artifact"];
    let page = f.call(json!({"op":"output","id":artifact["id"],"offset":0,"limit":65536}));
    assert_eq!(page["eof"], true);
    let data: Value = serde_json::from_str(page["text"].as_str().unwrap()).unwrap();
    assert_eq!(data["text"], vec!["x".repeat(4096); 8].join("\n"));
    assert_eq!(data["aggregation"], "native_message");
    let events = f.call(json!({"op":"read","target":id,
        "after":reply["result"]["events_after"],"limit":200}));
    let artifacts: Vec<_> = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|event| event["data"].get("artifact"))
        .collect();
    assert_eq!(artifacts, vec![artifact]);
    assert_eq!(
        fs::read_dir(f.root.join("runtime/output")).unwrap().count(),
        1
    );
    f.shutdown();
}

provider_matrix!(
    external_calls_timeout_resume_dedup_and_preserve_exact_turn_on_both_providers,
    external_calls_timeout_resume_dedup_and_preserve_exact_turn_on_both_providers_case
);

fn external_calls_timeout_resume_dedup_and_preserve_exact_turn_on_both_providers_case(
    provider: Provider,
) {
    let claude_provider = provider == Provider::Claude;
    let mut f = if claude_provider {
        claude::fixture()
    } else {
        Harness::new()
    };
    f.start();
    let id = if claude_provider {
        claude::spawn(&f, "hold")
    } else {
        f.spawn("hold")
    };
    f.wait(&id, |a| a["state"] == "running");
    let input = f.tmp.path().join("input.txt");
    fs::write(&input, "reply:exact-message").unwrap();
    let args = [
        "send",
        &id,
        "--body-file",
        input.to_str().unwrap(),
        "--request-id",
        "external-call",
        "--wait",
        "--timeout",
        "1",
    ];
    let started = Instant::now();
    let (pending, output) = cli(&f, &args);
    assert_eq!(pending["outcome"], "timeout", "{pending}");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(pending["result"]["delivery_state"], "accepted");
    assert_eq!(pending["result"]["queued_ahead"], 0);
    assert_eq!(pending["result"]["active_input"], true);
    assert!(String::from_utf8_lossy(&output.stderr).contains("queued behind 0"));
    let message = pending["message_id"].as_str().unwrap();
    let (retried, _) = cli(&f, &args);
    assert_eq!(retried["message_id"], message);
    let messages = f.mailbox(&id);
    assert_eq!(messages["messages"].as_array().unwrap().len(), 2);
    f.ok(json!({"op":"interrupt","target":id}));
    let (reply, output) = cli(&f, &["wait", "--message", message, "--timeout", "10"]);
    assert!(output.status.success(), "{reply}");
    assert_eq!(reply["outcome"], "replied");
    assert_eq!(
        reply["result"]["output"]["text"],
        if claude_provider {
            "answer:reply:exact-message"
        } else {
            "reply:exact-message"
        }
    );
    assert_eq!(reply["result"]["output_partial"], false);
    f.ok(json!({"op":"send","target":id,"body":"fail"}));
    f.wait(&id, |a| {
        a["state"] == if claude_provider { "idle" } else { "failed" }
    });
    let later = f.call(json!({"op":"message_result","message_id":message}));
    assert_eq!(later["turn_id"], reply["result"]["turn_id"]);
    assert_eq!(later["output"], reply["result"]["output"]);
    assert_eq!(later["outcome"], "replied");
    f.shutdown();
    f.start();
    let restored = f.call(json!({"op":"message_result","message_id":message}));
    assert_eq!(restored["output"], later["output"]);
    f.shutdown();
    let compacted = runtime::compact(&f.root, hive::state::now() + 1, 1).unwrap();
    assert!(compacted.call_results_removed >= 1);
    f.start();
    let (expired, _) = cli(&f, &["wait", "--message", message, "--timeout", "1"]);
    assert_eq!(expired["message_id"], message);
    assert_eq!(expired["error"]["code"], "result_expired", "{expired}");
    f.shutdown();
}

#[test]
fn external_call_rejects_steer_before_mutation_and_redirect_waits_for_its_own_turn() {
    let mut f = Harness::new();
    f.start();
    let id = f.spawn("hold");
    f.wait(&id, |a| a["state"] == "running");
    let file = f.tmp.path().join("guidance");
    fs::write(&file, "reply:redirected").unwrap();
    let (rejected, _) = cli(
        &f,
        &[
            "send",
            &id,
            "--body-file",
            file.to_str().unwrap(),
            "--wait",
            "--mode",
            "steer",
        ],
    );
    assert_eq!(rejected["error"]["code"], "conflicting_arguments");
    assert_eq!(rejected["error"]["field"], "--mode");
    assert_eq!(
        f.call(json!({"op":"messages","target":id}))["messages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let (reply, output) = cli(
        &f,
        &[
            "send",
            &id,
            "--body-file",
            file.to_str().unwrap(),
            "--wait",
            "--mode",
            "redirect",
            "--timeout",
            "10",
        ],
    );
    assert!(output.status.success(), "{reply}");
    assert_eq!(reply["result"]["output"]["text"], "reply:redirected");
    assert_eq!(reply["outcome"], "replied");
    f.shutdown();
}

#[test]
fn cancelling_agent_marks_queued_external_calls_cancelled_not_successful() {
    let mut f = Harness::new();
    f.start();
    let id = f.spawn("hold");
    f.wait(&id, |a| a["state"] == "running");
    let send = f.call(json!({"op":"send","target":id,"body":"queued"}));
    f.ok(json!({"op":"terminate","target":id}));
    let (result, output) = cli(
        &f,
        &[
            "wait",
            "--message",
            send["message_id"].as_str().unwrap(),
            "--timeout",
            "3",
        ],
    );
    assert!(!output.status.success());
    assert_eq!(result["outcome"], "cancelled");
    assert_eq!(result["result"]["delivery_state"], "undeliverable");
    f.shutdown();
}

#[test]
fn lost_acceptance_ack_can_retry_same_request_with_a_different_wait_timeout() {
    let mut f = Harness::new();
    f.start();
    let id = f.spawn("hold");
    f.wait(&id, |a| a["state"] == "running");
    let (proxy, forward) = lost_ack_proxy(&f);
    let input = f.tmp.path().join("request");
    fs::write(&input, "reply:lost-ack").unwrap();
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--state-dir")
        .arg(proxy)
        .args([
            "send",
            &id,
            "--body-file",
            input.to_str().unwrap(),
            "--wait",
            "--request-id",
            "lost-ack",
            "--timeout",
            "3",
        ])
        .output()
        .unwrap();
    let failed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(failed["outcome"], "observation_unavailable");
    assert_eq!(failed["request_id"], "lost-ack");
    let committed = forward.join().unwrap();
    let (retried, _) = cli(
        &f,
        &[
            "send",
            &id,
            "--body-file",
            input.to_str().unwrap(),
            "--wait",
            "--request-id",
            "lost-ack",
            "--timeout",
            "1",
        ],
    );
    assert_eq!(retried["outcome"], "timeout");
    assert_eq!(retried["message_id"], committed["message_id"]);
    assert_eq!(
        f.call(json!({"op":"messages","target":id}))["messages"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    f.shutdown();
}

fn lost_ack_proxy(f: &Harness) -> (PathBuf, support::ThreadGuard<Value>) {
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;
    let proxy = f.tmp.path().join("proxy");
    fs::create_dir_all(proxy.join("runtime")).unwrap();
    let listener = UnixListener::bind(runtime::socket_path(&proxy)).unwrap();
    let root = f.root.clone();
    let wake_path = runtime::socket_path(&proxy);
    let forward = support::ThreadGuard::new(
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            let committed = runtime::client(
                &root,
                request["id"].as_str().unwrap(),
                request["args"].clone(),
            )
            .unwrap();
            drop(stream); // Durable acceptance, deliberately lost response.
            committed
        }),
        move || {
            let _ = std::os::unix::net::UnixStream::connect(wake_path);
        },
    );
    (proxy, forward)
}
