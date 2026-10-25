use super::*;

fn question(request: &str, schema: bool) -> String {
    let mut args = json!({"request_id":request,"message":"Select deployment settings","title":"Deploy staging","destructive":true});
    if schema {
        args["schema"] = json!({"type":"object","properties":{"region":{"type":"string","enum":["eu","us"]},"replicas":{"type":"integer","minimum":1,"maximum":3}},"required":["region"]});
    }
    format!("ask:{args}")
}
fn cli(f: &Harness, args: &[&str]) -> (bool, Value) {
    let output = support::command(env!("CARGO_BIN_EXE_hivectl"))
        .arg("--state-dir")
        .arg(&f.root)
        .args(args)
        .output()
        .unwrap();
    let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "{} / {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.success(), value)
}
fn pending(f: &Harness, id: &str) -> Value {
    f.wait(id, |a| a["state"] == "idle" && a["waiting_on"] == "human")
}
fn wait_message(f: &Harness, accepted: &Value) -> Value {
    let (ok, result) = cli(
        f,
        &[
            "wait",
            "--message",
            accepted["message_id"].as_str().unwrap(),
            "--timeout",
            "15",
        ],
    );
    assert!(ok, "{result}");
    assert_eq!(result["outcome"], "replied", "{result}");
    result
}
fn inspect_form(f: &Harness, id: &str, approval: &str) {
    let (ok, page) = cli(f, &["approvals", "--agent", id, "--wait", "--timeout", "2"]);
    assert!(ok, "{page}");
    assert_eq!(page["approvals"][0]["id"], approval);
    let (ok, detail) = cli(f, &["approvals", "show", approval]);
    assert!(ok, "{detail}");
    assert_eq!(detail["approval"]["schema"]["required"], json!(["region"]));
    let (ok, bad) = cli(
        f,
        &[
            "respond",
            approval,
            "--accept",
            "--data",
            r#"{"region":"unknown"}"#,
        ],
    );
    assert!(!ok);
    assert_eq!(bad["error"]["code"], "invalid_value");
    assert_eq!(bad["error"]["field"], "--data");
}
fn scenario(is_claude: bool) {
    let mut f = claude::fixture();
    f.start();
    let id = if is_claude {
        claude::spawn(&f, &question("form", true))
    } else {
        f.spawn(&question("form", true))
    };
    let first = pending(&f, &id);
    let approval = first["approval_id"].as_str().unwrap().to_owned();
    inspect_form(&f, &id, &approval);
    // Pending approvals do not hold the mailbox or become answered by ordinary input.
    let unrelated = f.call(json!({"op":"send","target":id,"body":"reply:unrelated"}));
    wait_message(&f, &unrelated);
    assert_eq!(pending(&f, &id)["approval_id"], approval);
    // Stop and confirm owned cleanup before a deliberate daemon restart.
    f.ok(json!({"op":"stop","target":id}));
    f.wait(&id, |a| a["state"] == "stopped");
    // Restart while pending, then answer on the original native conversation.
    f.shutdown();
    f.start();
    answer_after_restart(&f, &id, &approval);
    let idle = f.wait(&id, |a| a["state"] == "idle");
    assert_eq!(idle["thread_id"], first["thread_id"]);
    assert!(idle.get("waiting_on").is_none());
    let messages = f.mailbox(&id);
    let answers: Vec<_> = messages["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["kind"] == "approval_answer")
        .collect();
    assert_eq!(answers.len(), 1);
    assert!(answers[0].get("mode").is_none());
    let events = f.read(&id);
    assert!(
        !events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "redirect_accepted")
    );
    for (request, action) in [("second", "--decline"), ("third", "--cancel")] {
        f.ok(json!({"op":"send","target":id,"body":question(request,false)}));
        let a = pending(&f, &id);
        let key = a["approval_id"].as_str().unwrap();
        let (ok, answer) = cli(&f, &["respond", key, action]);
        assert!(ok, "{answer}");
        wait_message(&f, &answer);
    }
    let (ok, history) = cli(&f, &["approvals", "--agent", &id, "--all"]);
    assert!(ok);
    assert_eq!(history["approvals"].as_array().unwrap().len(), 3);
    let (ok, timeout) = cli(
        &f,
        &["approvals", "--agent", &id, "--wait", "--timeout", "1"],
    );
    assert!(!ok);
    assert_eq!(timeout["error"]["code"], "timeout");
    f.ok(json!({"op":"terminate","target":id}));
    f.shutdown();
}
#[test]
fn codex_approvals_cli_and_durable_round_trip() {
    scenario(false);
}
#[test]
fn claude_approvals_cli_and_durable_round_trip() {
    scenario(true);
}

#[test]
fn approvals_wait_observes_later_question_without_holding_daemon_writer() {
    let mut f = Harness::new();
    f.start();
    let waiter = support::ChildGuard::new(
        support::command(env!("CARGO_BIN_EXE_hivectl"))
            .arg("--state-dir")
            .arg(&f.root)
            .args(["approvals", "--wait", "--timeout", "10"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let id = f.spawn(&question("first", false));
    let approval = pending(&f, &id)["approval_id"].clone();
    let output = waiter.wait_with_stdout().unwrap();
    assert!(output.status.success());
    let page: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(page["approvals"][0]["id"], approval);
    f.ok(json!({"op":"terminate","target":id}));
}

provider_matrix!(
    approvals_do_not_interrupt_active_input_and_pause_holds_answer_until_resume,
    approvals_do_not_interrupt_active_input_and_pause_holds_answer_until_resume_case
);

fn approvals_do_not_interrupt_active_input_and_pause_holds_answer_until_resume_case(
    provider: Provider,
) {
    let is_claude = provider == Provider::Claude;
    let mut f = claude::fixture();
    f.start();
    let id = if is_claude {
        claude::spawn(&f, &question("active", false))
    } else {
        f.spawn(&question("active", false))
    };
    let approval = pending(&f, &id)["approval_id"].clone();
    f.ok(json!({"op":"send","target":id,"body":"hold"}));
    let running = f.wait(&id, |a| a["state"] == "running");
    let response = f.call(json!({"op":"respond","approval_id":approval,"action":"accept"}));
    assert!(response["message_id"].is_string(), "{response}");
    let state = f.call(json!({"op":"status","target":id}));
    assert_eq!(state["agent"]["turn_id"], running["turn_id"]);
    assert_eq!(state["agent"]["state"], "running");
    let message = f.call(json!({"op":"message_result","message_id":response["message_id"]}));
    assert_eq!(message["delivery_state"], "accepted");
    let sql = rusqlite::Connection::open(f.root.join("runtime/runtime.sqlite")).unwrap();
    let interruptions: i64 = sql
        .query_row("SELECT count(*) FROM redirect_interrupts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(interruptions, 0);
    // Only the explicit human interrupt ends the held turn.
    f.ok(json!({"op":"interrupt","target":id}));
    wait_message(&f, &response);
    f.ok(json!({"op":"send","target":id,"body":question("paused",false)}));
    let pending_state = pending(&f, &id);
    f.ok(json!({"op":"stop","target":id}));
    let stopped = f.wait(&id, |a| a["state"] == "stopped");
    assert_eq!(stopped["paused"], true);
    assert_eq!(stopped["waiting_on"], "human");
    let response = f
        .call(json!({"op":"respond","approval_id":pending_state["approval_id"],"action":"cancel"}));
    let stopped = f.call(json!({"op":"status","target":id}));
    assert_eq!(stopped["agent"]["state"], "stopped");
    assert_eq!(stopped["agent"]["paused"], true);
    let resumed = f.call(json!({"op":"resume","target":id}));
    assert!(resumed["error"].is_null(), "{resumed}");
    wait_message(&f, &response);
    f.wait(&id, |a| a["state"] == "idle");
    f.ok(json!({"op":"terminate","target":id}));
    f.wait(&id, |a| a["state"] == "stopped");
    f.shutdown();
}

#[test]
fn approvals_cli_rejects_malformed_after_cursor_before_contacting_daemon() {
    let f = Harness::new();
    let (ok, result) = cli(&f, &["approvals", "--after", "{broken"]);
    assert!(!ok);
    assert_eq!(result["error"]["code"], "invalid_json");
    assert_eq!(result["error"]["field"], "--after");
}

fn answer_after_restart(f: &Harness, id: &str, approval: &str) {
    let data = f.tmp.path().join("answer.json");
    fs::write(&data, r#"{"region":"eu","replicas":2}"#).unwrap();
    let args = [
        "respond",
        approval,
        "--accept",
        "--data-file",
        data.to_str().unwrap(),
        "--request-id",
        "human-answer",
    ];
    let (ok, answer) = cli(f, &args);
    assert!(ok, "{answer}");
    let (ok, retried) = cli(f, &args);
    assert!(ok);
    assert_eq!(retried, answer);
    let resumed = f.call(json!({"op":"resume","target":id}));
    assert!(resumed["error"].is_null(), "{resumed}");
    let message = answer["message_id"].as_str().unwrap();
    let (ok, reply) = cli(f, &["wait", "--message", message, "--timeout", "10"]);
    assert!(
        ok,
        "{reply}; status={}; events={}",
        f.call(json!({"op":"status","target":id})),
        f.call(json!({"op":"read","target":id,"limit":200}))
    );
    assert_eq!(reply["outcome"], "replied", "{reply}");
    assert!(
        reply["result"]["output"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("Approval ")),
        "{reply}"
    );
}
