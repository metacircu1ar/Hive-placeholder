//! `hivectl tui` driven through a real terminal against the mock daemon.
use super::*;

/// The default prefix, Ctrl-A, as a terminal sends it.
const PREFIX: &str = "\u{1}";
const CTRL_C: &str = "\u{3}";

fn tui(f: &Harness, steps: &Value) -> std::process::Child {
    support::command(support::python())
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pty_attach.py"))
        .arg(cli_binary())
        .arg("--state-dir")
        .arg(&f.root)
        .arg("--config")
        .arg(&f.config)
        .arg("tui")
        // Dialog spawns default to this absolute directory.
        .current_dir(f.tmp.path())
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

fn short(id: &str) -> &str {
    &id[..8]
}

#[test]
fn two_agents_open_side_by_side_from_the_sidebar() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let first = f.spawn("reply: first agent");
    let second = f.spawn("reply: second agent");
    for id in [&first, &second] {
        f.wait(id, |a| a["state"] == "idle" && a["turn_id"].is_null());
    }
    let steps = json!([
        ["wait", "agents 2"],
        // Typing filters the sidebar; Enter opens the match in a new tab.
        ["send", format!("{}\r", short(&first))],
        ["wait", "human: reply: first agent"],
        // Prefix v splits the tab with the next agent picked.
        ["send", format!("{PREFIX}v")],
        ["wait", "pick an agent for the split"],
        ["send", format!("{}\r", short(&second))],
        ["wait", "human: reply: second agent"],
        ["wait", "human: reply: first agent"],
        // The right view is active: typing there reaches the second agent.
        ["send", "reply: typed in the split\r"],
        ["wait", "human: reply: typed in the split"],
        ["send", CTRL_C],
    ]);
    let result = finished(tui(&f, &steps));
    assert_eq!(result["status"], 0, "{result}");
    let bodies: Vec<String> = f.read(&second)["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["type"] == "message_accepted")
        .map(|event| event["data"]["body"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        bodies
            .iter()
            .any(|body| body == "reply: typed in the split"),
        "{bodies:?}"
    );
    f.shutdown();
}

#[test]
fn a_question_is_answered_from_the_attention_queue() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let ask = json!({"request_id": "deploy", "message": "Approve the deploy?"});
    let id = f.spawn(&format!("ask:{ask}"));
    let pending = f.wait(&id, |a| a["state"] == "idle" && a["waiting_on"] == "human");
    let approval = pending["approval_id"].as_str().unwrap().to_owned();
    let steps = json!([
        ["wait", "! 1 need you"],
        ["send", format!("{PREFIX}a")],
        ["wait", "needs you (1)"],
        // Enter opens the agent with its question card focused.
        ["send", "\r"],
        ["wait", "Approve the deploy?"],
        ["send", "a"],
        ["wait", "accept: ok"],
        ["gone", "need you"],
        ["send", CTRL_C],
    ]);
    let result = finished(tui(&f, &steps));
    assert_eq!(result["status"], 0, "{result}");
    let detail = f.ok(json!({"op": "approval", "approval_id": approval}));
    assert_eq!(detail["approval"]["action"], "accept", "{detail}");
    f.shutdown();
}

#[test]
fn the_dialog_spawns_an_agent_and_opens_it() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let steps = json!([
        ["wait", "no live agents"],
        ["send", format!("{PREFIX}c")],
        ["wait", "spawn an agent"],
        ["wait", "Tab next field"],
        ["send", "reply: from the dialog"],
        // Tab from the task wraps to the role; → picks the first role.
        ["send", "\t\u{1b}[C"],
        ["wait", "‹ implementor ›"],
        ["send", "\r"],
        ["wait", "human: reply: from the dialog"],
        ["wait", "agents 1"],
        ["send", CTRL_C],
    ]);
    let result = finished(tui(&f, &steps));
    assert_eq!(result["status"], 0, "{result}");
    let agents = f.ok(json!({"op": "agents"}));
    let rows = agents["agents"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{agents}");
    assert_eq!(rows[0]["role"], "implementor");
    assert_eq!(
        f.status(rows[0]["id"].as_str().unwrap())["cwd"],
        json!(f.tmp.path().canonicalize().unwrap())
    );
    f.shutdown();
}

#[test]
fn closing_a_view_leaves_its_agent_running() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let id = f.spawn("hold");
    f.wait(&id, |a| a["state"] == "running");
    let steps = json!([
        ["wait", "agents 1"],
        ["send", format!("{}\r", short(&id))],
        ["wait", "human: hold"],
        ["send", format!("{PREFIX}w")],
        ["wait", "the agent keeps running"],
        ["wait", "No agent view is open"],
        ["send", CTRL_C],
    ]);
    let result = finished(tui(&f, &steps));
    assert_eq!(result["status"], 0, "{result}");
    let status = f.status(&id);
    assert_eq!(status["state"], "running", "{status}");
    f.ok(json!({"op": "interrupt", "target": id}));
    f.shutdown();
}

#[test]
fn the_tui_refuses_without_a_terminal_or_with_an_invalid_prefix() {
    let f = Harness::builder(Provider::Codex).build();
    let output = support::command(cli_binary())
        .arg("--state-dir")
        .arg(&f.root)
        .arg("tui")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        error["error"]["code"], "interactive_terminal_required",
        "{error}"
    );
    std::fs::write(f.config.with_file_name("tui.toml"), "prefix = \"ctrl-c\"\n").unwrap();
    let result = finished(tui(&f, &json!([["wait", "config_invalid"]])));
    assert_eq!(result["status"], 1, "{result}");
    assert!(result["screen"].as_str().unwrap().contains("prefix"));
}

#[test]
fn the_tui_exits_when_its_terminal_hangs_up() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let id = f.spawn("hold");
    f.wait(&id, |a| a["state"] == "running");
    // A view's poll, the sidebar's poll and the event reader are all waiting.
    let steps = json!([
        ["wait", "agents 1"],
        ["send", format!("{}\r", short(&id))],
        ["wait", "human: hold"],
        ["hangup", 5],
    ]);
    let result = finished(tui(&f, &steps));
    assert_eq!(result["status"], 1, "{result}");
    f.ok(json!({"op": "interrupt", "target": id}));
    f.shutdown();
}

#[test]
fn the_mouse_opens_agents_from_the_sidebar_and_scrolls_a_view() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let first = f.spawn("reply: first agent");
    // A reply taller than the view: forty list items.
    let items: Vec<String> = (1..=40).map(|n| format!("- item-{n:02}")).collect();
    let second = f.spawn(&format!("reply:\n\n{}", items.join("\n")));
    for id in [&first, &second] {
        f.wait(id, |a| a["state"] == "idle" && a["turn_id"].is_null());
    }
    let steps = json!([
        ["wait", "agents 2"],
        // A click on the second agent's row opens its view.
        ["click", short(&second)],
        ["wait", "• item-40"],
        ["absent", "• item-15"],
        // Wheel steps over the view bring back older items.
        ["wheel", "• item-40", 5],
        ["wait", "• item-15"],
        // And a click on the first agent's row opens that one.
        ["click", short(&first)],
        ["wait", "human: reply: first agent"],
        ["send", CTRL_C],
    ]);
    let result = finished(tui(&f, &steps));
    assert_eq!(result["status"], 0, "{result}");
    f.shutdown();
}

/// macOS revokes a closed window's terminal instead of hanging it up.
#[cfg(target_os = "macos")]
#[test]
fn the_tui_exits_when_its_terminal_is_revoked() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let id = f.spawn("reply: before the window closed");
    f.settled(&id);
    let steps = json!([
        ["wait", "agents 1"],
        ["send", format!("{}\r", short(&id))],
        ["wait", "before the window closed"],
        ["revoke", 5],
    ]);
    let result = finished(tui(&f, &steps));
    assert_eq!(result["status"], 1, "{result}");
    f.shutdown();
}

#[test]
fn a_question_is_answered_by_clicking_accept() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let ask = json!({"request_id": "click", "message": "Approve by mouse?"});
    let id = f.spawn(&format!("ask:{ask}"));
    let pending = f.wait(&id, |a| a["state"] == "idle" && a["waiting_on"] == "human");
    let approval = pending["approval_id"].as_str().unwrap().to_owned();
    let steps = json!([
        ["wait", "! 1 need you"],
        ["click", short(&id)],
        ["wait", " Accept "],
        ["click", " Accept "],
        ["wait", "accept: ok"],
        ["send", CTRL_C],
    ]);
    let result = finished(tui(&f, &steps));
    assert_eq!(result["status"], 0, "{result}");
    let detail = f.ok(json!({"op": "approval", "approval_id": approval}));
    assert_eq!(detail["approval"]["action"], "accept", "{detail}");
    f.shutdown();
}

#[test]
fn the_quit_button_leaves_the_tui() {
    let mut f = Harness::builder(Provider::Codex).build();
    f.start();
    let steps = json!([["wait", " Quit "], ["click", " Quit "]]);
    let result = finished(tui(&f, &steps));
    assert_eq!(result["status"], 0, "{result}");
    f.shutdown();
}
