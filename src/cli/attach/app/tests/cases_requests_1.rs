//! Requests regressions.
use super::*;

#[test]
fn every_action_gets_its_own_request_id_and_the_input_clears() {
    let mut app = app("idle", false);
    for text in ["one", "two"] {
        typed(&mut app, text);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    }
    let sent = writes(&mut app);
    assert_eq!(sent.len(), 2);
    assert_ne!(sent[0].1, sent[1].1);
    assert_eq!(
        (sent[0].2["body"].clone(), sent[1].2["body"].clone()),
        (json!("one"), json!("two"))
    );
    assert!(app.input.is_empty());
    // An empty input sends nothing.
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(writes(&mut app).is_empty());
}

#[test]
fn scrolling_to_the_top_requests_older_history_once() {
    let mut app = app("idle", false);
    app.shown = Shown {
        lines: 30,
        height: 10,
        more: false,
        ..Shown::default()
    };
    press(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
    press(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
    press(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
    let history: Vec<_> = app
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::History { before } => Some(*before),
            Command::Write { .. } | Command::Usage { .. } => None,
        })
        .collect();
    assert_eq!(history, [4]);
    assert_eq!(app.scroll, 20);
}

#[test]
fn without_listed_names_the_commands_show_their_arguments() {
    // hivectl attach lists nothing.
    let mut app = app("idle", false);
    run(&mut app, "/model");
    assert_eq!(app.status, "usage: /model MODEL");
    assert_eq!(app.input, "/model");
    // A picker needs the agent's snapshot.
    let mut app = App::new(String::from("a"), Mode::default());
    run(&mut app, "/effort");
    assert_eq!(app.status, "usage: /effort EFFORT");
    // An empty list for the agent's provider, or a provider not yet known.
    let mut app = listed_app(&[]);
    run(&mut app, "/model");
    assert_eq!(app.status, "usage: /model MODEL");
    let mut app = listed_app(&["gpt-6"]);
    app.update(Update::Snapshot(json!({
        "agent": {"id": "a", "state": "idle", "task_outcome": "open", "model": "m",
                  "effort": "low", "auth": "main", "steering_supported": false, "ended_at": null},
        "events": {"events": [], "cursor": 0, "pruned_through": 0, "history_expired": false},
        "queued": 0, "mail": 0, "children": 0,
    })));
    run(&mut app, "/effort");
    assert_eq!(app.status, "usage: /effort EFFORT");
    assert!(card_text(&app).is_none());
}
