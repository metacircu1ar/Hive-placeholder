//! Behaviour regressions.
use super::*;

#[test]
fn enter_follows_the_agent_state_and_modifiers_choose_the_mode() {
    let modes = |state, steering, code, modifiers| {
        let mut app = app(state, steering);
        typed(&mut app, "hello");
        press(&mut app, code, modifiers);
        writes(&mut app)
            .pop()
            .map(|(_, _, request)| request["mode"].clone())
    };
    let none = KeyModifiers::NONE;
    assert_eq!(
        modes("idle", true, KeyCode::Enter, none),
        Some(json!("next-turn"))
    );
    assert_eq!(
        modes("running", true, KeyCode::Enter, none),
        Some(json!("steer"))
    );
    assert_eq!(
        modes("running", false, KeyCode::Enter, none),
        Some(json!("next-turn"))
    );
    assert_eq!(
        modes("running", true, KeyCode::Enter, KeyModifiers::ALT),
        Some(json!("next-turn"))
    );
    assert_eq!(
        modes("running", true, KeyCode::Char('r'), KeyModifiers::CONTROL),
        Some(json!("redirect"))
    );
}

#[test]
fn double_escape_interrupts_only_from_the_input() {
    let mut app = app("running", true);
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    press(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(writes(&mut app).is_empty());
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(
        writes(&mut app)[0].2,
        json!({"op": "interrupt", "target": "a"})
    );
    // With a question focused, Esc returns to the input and never interrupts.
    app.update(Update::Approval(
        json!({"id": "q", "state": "pending", "message": "Go?"}),
    ));
    press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(app.focus, Focus::Card);
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(app.focus, Focus::Input);
    assert!(writes(&mut app).is_empty());
}

#[test]
fn control_c_confirms_while_a_write_is_in_flight() {
    let mut app = app("idle", false);
    typed(&mut app, "hello");
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    let (action, ..) = writes(&mut app).remove(0);
    press(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(app.exit.is_none() && app.status.contains("in flight"));
    press(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(app.exit, Some(Exit::Quit));
    // Once the write is answered, one Ctrl-C quits.
    let mut app = self::app("idle", false);
    app.update(Update::Done {
        action,
        label: String::from("send"),
        result: Ok(json!({})),
    });
    press(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(app.exit, Some(Exit::Quit));
}

#[test]
fn slash_commands_map_to_operations() {
    let command = |line: &str| {
        let mut app = app("idle", false);
        typed(&mut app, line);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        (writes(&mut app).pop().map(|(_, _, request)| request), app)
    };
    assert_eq!(
        command("/model gpt-6").0,
        Some(json!({"op": "set_options", "target": "a", "options": {"model": "gpt-6"}}))
    );
    assert_eq!(
        command("/name diff reviewer").0,
        Some(json!({"op": "set_name", "target": "a", "name": "diff reviewer"}))
    );
    let (none, app) = command("/name");
    assert!(
        none.is_none() && app.status == "usage: /name NAME",
        "{}",
        app.status
    );
    assert_eq!(
        command("/options effort=high verbose=true").0,
        Some(json!({"op": "set_options", "target": "a",
                    "options": {"effort": "high", "verbose": true}}))
    );
    assert_eq!(
        command("/complete Done here").0,
        Some(json!({"op": "complete", "target": "a", "body": "Done here"}))
    );
    assert_eq!(
        command("/branch Try another way").0,
        Some(json!({"op": "spawn", "from_agent": "a", "body": "Try another way"}))
    );
    let (spawn, _) = command("/spawn reviewer Check it");
    let spawn = spawn.unwrap();
    assert_eq!(
        (&spawn["role"], &spawn["body"], spawn["cwd"].is_string()),
        (&json!("reviewer"), &json!("Check it"), true)
    );
    assert_eq!(
        command("/terminate").0,
        Some(json!({"op": "terminate", "target": "a"}))
    );
    let (none, app) = command("/attach child-1");
    assert!(none.is_none());
    assert_eq!(app.exit, Some(Exit::Attach(String::from("child-1"))));
    // An incomplete command keeps the input and shows usage.
    let (none, app) = command("/spawn reviewer");
    assert!(none.is_none() && app.status.starts_with("usage") && app.input == "/spawn reviewer");
}

#[test]
fn other_slash_commands_are_forwarded_and_double_slash_sends_literally() {
    let sent = |text: &str, state: &str| {
        let mut app = app(state, true);
        typed(&mut app, text);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let status = app.status.clone();
        let input = app.input.clone();
        (
            writes(&mut app).pop().map(|(_, _, request)| request),
            status,
            input,
        )
    };
    let (request, status, input) = sent("/review src/", "idle");
    assert_eq!(
        request,
        Some(json!({"op": "send", "target": "a", "body": "/review src/", "mode": "next-turn"}))
    );
    assert_eq!(
        (status.as_str(), input.as_str()),
        ("forward /review to the agent…", "")
    );
    // A forwarded line follows the same mode rule as a message.
    let (request, _, _) = sent("/tmp/x is broken", "running");
    assert_eq!(request.unwrap()["mode"], "steer");
    let (request, _, _) = sent("//model is a slash command", "idle");
    assert_eq!(request.unwrap()["body"], "/model is a slash command");
    // A view command with the wrong arguments is never forwarded.
    for (text, hint) in [
        ("/model", "usage: /model MODEL"),
        ("/stop now", "usage: /stop"),
        ("/usage codex-main", "usage: /usage"),
        ("/quit please", "usage: /quit"),
        ("/complete", "usage: /complete TEXT"),
        ("/attach", "usage: /attach AGENT"),
        ("/", "usage: /help lists the view commands"),
        ("/ text", "usage: /help lists the view commands"),
    ] {
        let (request, status, input) = sent(text, "idle");
        assert_eq!(
            (request, status.as_str(), input.as_str()),
            (None, hint, text)
        );
    }
}

#[test]
fn exit_on_end_leaves_after_the_linger() {
    let mut app = App::new(
        String::from("a"),
        Mode {
            exit_on_end: true,
            ..Mode::default()
        },
    );
    let mut ended = snapshot("idle", false);
    if let Update::Snapshot(reply) = &mut ended {
        reply["agent"]["ended_at"] = json!(10);
    }
    app.update(ended);
    let started = Instant::now();
    app.tick(started);
    assert!(app.exit.is_none());
    app.tick(started + END_LINGER + Duration::from_millis(10));
    assert_eq!(app.exit, Some(Exit::Quit));
    // A purged agent also counts as ended.
    let mut app = App::new(
        String::from("a"),
        Mode {
            exit_on_end: true,
            ..Mode::default()
        },
    );
    app.update(Update::Purged);
    app.tick(Instant::now() + END_LINGER + Duration::from_millis(10));
    assert_eq!(app.exit, Some(Exit::Quit));
}

#[test]
fn failed_history_pages_can_be_retried_and_refusals_are_shown() {
    let mut app = app("idle", false);
    app.shown = Shown {
        lines: 30,
        height: 10,
        more: false,
        ..Shown::default()
    };
    press(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
    press(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
    assert_eq!(app.commands.len(), 1);
    app.commands.clear();
    app.update(Update::History(Err(String::from(
        "daemon_stopping: stopping",
    ))));
    assert!(app.status.contains("older history: daemon_stopping"));
    // The failure released the request, so scrolling up asks again.
    press(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
    assert_eq!(app.commands.len(), 1);
    app.update(Update::Problem(String::from("invalid_value: /after")));
    assert_eq!(app.status, "invalid_value: /after");
}

#[test]
fn the_linger_follows_the_mode() {
    let mode = Mode {
        exit_on_end: true,
        linger: Duration::ZERO,
        ..Mode::default()
    };
    let mut app = App::new(String::from("a"), mode);
    app.update(Update::Purged);
    app.tick(Instant::now());
    assert_eq!(app.exit, Some(Exit::Quit));
}
