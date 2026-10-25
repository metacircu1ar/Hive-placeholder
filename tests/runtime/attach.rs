//! `hivectl attach` driven through a real terminal against the mock daemon.
use super::*;
use support::wait::{POLL_INTERVAL, TEST_TIMEOUT, wait_until};

const ESC: &str = "\u{1b}";
const CTRL_C: &str = "\u{3}";
const CTRL_R: &str = "\u{12}";

/// Run the view under a terminal with these steps; returns the driver's result.
fn view(f: &Harness, id: &str, flags: &[&str], steps: &Value) -> std::process::Child {
    support::command(support::python())
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pty_attach.py"))
        .arg(cli_binary())
        .arg("--state-dir")
        .arg(&f.root)
        .args(["attach", id])
        .args(flags)
        .env("HIVE_PTY_STEPS", steps.to_string())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap()
}

fn finished(child: std::process::Child) -> Value {
    let output = child.wait_with_output().unwrap();
    let result: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&output.stdout)));
    assert_eq!(
        result["failed_step"],
        Value::Null,
        "{}",
        result["screen"].as_str().unwrap()
    );
    result
}

fn accepted(f: &Harness, id: &str) -> Vec<(String, String)> {
    f.read(id)["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["type"] == "message_accepted" || event["type"] == "redirect_accepted")
        .map(|event| {
            let data = &event["data"];
            (
                data["body"].as_str().unwrap_or_default().to_owned(),
                data["mode"].as_str().unwrap_or("next-turn").to_owned(),
            )
        })
        .collect()
}

#[test]
fn the_view_sends_messages_and_leaves_after_the_agent_ends() {
    let mut f = Harness::builder(Provider::Codex)
        .scenario(json!({"live_deltas": true}))
        .build();
    f.start();
    let id = f.spawn("reply: first");
    f.settled(&id);
    let steps = json!([
        ["wait", "reply: first"],
        ["send", "reply: hello there\r"],
        ["wait", "human: reply: hello there"],
        ["wait", "── completed"],
        ["send", "/terminate\r"],
        ["wait", "ended"],
    ]);
    let result = finished(view(&f, &id, &["--exit-on-end"], &steps));
    // --exit-on-end leaves on its own after the linger.
    assert_eq!(result["status"], 0, "{result}");
    let sent: Vec<_> = accepted(&f, &id)
        .into_iter()
        .filter(|(body, _)| body == "reply: hello there")
        .collect();
    assert_eq!(
        sent,
        [(
            String::from("reply: hello there"),
            String::from("next-turn")
        )]
    );
    f.shutdown();
}

#[test]
fn keys_steer_redirect_and_interrupt_a_running_turn() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let id = f.spawn("hold");
    f.wait(&id, |a| {
        a["state"] == "running" && a["steering_supported"] == true
    });
    let steps = json!([
        ["wait", "running"],
        // Enter steers a running turn that supports steering.
        ["send", "guidance one\r"],
        ["wait", "steered: guidance one"],
        // Ctrl-R redirects: the held turn ends and this text runs first.
        ["send", format!("reply: redirected{CTRL_R}")],
        ["wait", "human [redirect]: reply: redirected"],
        ["wait", "idle"],
        ["send", "hold\r"],
        ["wait", "running"],
        ["send", ESC],
        ["sleep", 0.15],
        ["send", ESC],
        ["wait", "interrupt: ok"],
        ["send", CTRL_C],
    ]);
    let result = finished(view(&f, &id, &[], &steps));
    assert_eq!(result["status"], 0, "{result}");
    // The redirect and the double Esc each interrupted one turn.
    let screen = result["screen"].as_str().unwrap();
    assert_eq!(screen.matches("── interrupted").count(), 2, "{screen}");
    let sent = accepted(&f, &id);
    for (body, mode) in [
        ("guidance one", "steer"),
        ("reply: redirected", "redirect"),
        ("hold", "next-turn"),
    ] {
        let matching: Vec<_> = sent.iter().filter(|(b, _)| b == body).collect();
        assert_eq!(matching, [&(body.to_owned(), mode.to_owned())], "{sent:?}");
    }
    f.shutdown();
}

#[test]
fn a_question_form_is_answered_from_its_card() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let ask = json!({"request_id": "form", "message": "Select deployment settings",
        "title": "Deploy staging", "schema": {"type": "object", "required": ["region"],
        "properties": {"region": {"type": "string", "enum": ["eu", "us"]},
                       "replicas": {"type": "integer", "minimum": 1, "maximum": 3}}}});
    let id = f.spawn(&format!("ask:{ask}"));
    let pending = f.wait(&id, |a| a["state"] == "idle" && a["waiting_on"] == "human");
    let approval = pending["approval_id"].as_str().unwrap().to_owned();
    let steps = json!([
        ["wait", "Deploy staging"],
        ["wait", "region*:"],
        ["send", "\t"],
        // Enter without a region is refused before anything is sent.
        ["send", "\r"],
        ["wait", "region is required"],
        ["send", "\u{1b}[C"],
        ["wait", "region*: eu"],
        ["send", "\r"],
        ["gone", "region*: eu"],
        ["send", CTRL_C],
    ]);
    let result = finished(view(&f, &id, &[], &steps));
    assert_eq!(result["status"], 0, "{result}");
    let detail = f.ok(json!({"op": "approval", "approval_id": approval}));
    assert_eq!(
        (
            &detail["approval"]["action"],
            &detail["approval"]["content"]
        ),
        (&json!("accept"), &json!({"region": "eu"}))
    );
    f.shutdown();
}

#[test]
fn a_daemon_restart_reconnects_without_losing_or_repeating_entries() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let id = f.spawn("reply: one");
    f.settled(&id);
    let steps = json!([
        ["wait", "human: reply: one"],
        ["wait", "disconnected"],
        ["gone", "disconnected"],
        // A daemon shutdown stops executions; resuming continues the conversation.
        ["send", "/resume\r"],
        ["wait", "resume: ok"],
        ["send", "reply: two\r"],
        ["wait", "human: reply: two"],
        ["wait", "── completed"],
        ["sleep", 0.5],
        ["send", CTRL_C],
    ]);
    let child = view(&f, &id, &[], &steps);
    std::thread::sleep(Duration::from_secs(3));
    f.shutdown();
    f.start();
    let result = finished(child);
    assert_eq!(result["status"], 0, "{result}");
    let screen = result["screen"].as_str().unwrap();
    assert_eq!(screen.matches("human: reply: one").count(), 1, "{screen}");
    assert_eq!(screen.matches("human: reply: two").count(), 1, "{screen}");
    f.shutdown();
}

#[test]
fn the_view_exits_when_its_terminal_hangs_up() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let id = f.spawn("reply: before the hangup");
    f.settled(&id);
    let steps = json!([["wait", "reply: before the hangup"], ["hangup", 5]]);
    let result = finished(view(&f, &id, &[], &steps));
    assert_eq!(result["status"], 1, "{result}");
    f.shutdown();
}

#[test]
fn attach_refuses_without_a_terminal() {
    let f = Harness::builder(Provider::Codex).build();
    let output = support::command(cli_binary())
        .arg("--state-dir")
        .arg(&f.root)
        .args(["attach", "agent"])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        error["error"]["code"], "interactive_terminal_required",
        "{error}"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("hivectl live"));
}

#[test]
fn an_unknown_agent_ends_the_view_with_the_daemon_error() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let steps = json!([["wait", "unknown_agent"]]);
    let result = finished(view(&f, "no-such-agent", &["--exit-on-end"], &steps));
    // Like any command's operational failure: exit 1 with the error record.
    assert_eq!(result["status"], 1, "{result}");
    assert!(
        result["screen"]
            .as_str()
            .unwrap()
            .contains("cannot attach to the agent")
    );
    f.shutdown();
}

#[test]
fn an_idle_agent_shows_its_newest_message_beyond_the_page_budget() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let id = f.spawn("reply: first");
    f.settled(&id);
    // Each turn stores about 30 KiB inline, so the newest 200 rows exceed the
    // 768 KiB page budget and the first attach page ends before the newest rows.
    let filler = "x".repeat(14 * 1024);
    for turn in 0..32 {
        f.ok(json!({"op":"send","target":id,"body":format!("reply: {filler} marker-{turn:02}")}));
    }
    wait_until(TEST_TIMEOUT, POLL_INTERVAL, "all turns answered", || {
        let events = f.read(&id);
        events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| {
                e["type"] == "assistant_message"
                    && e["data"]["text"]
                        .as_str()
                        .is_some_and(|t| t.ends_with("marker-31"))
            })
            .then_some(())
    });
    let steps = json!([["wait", "marker-31"], ["send", CTRL_C]]);
    let result = finished(view(&f, &id, &[], &steps));
    assert_eq!(result["status"], 0, "{result}");
    f.shutdown();
}
