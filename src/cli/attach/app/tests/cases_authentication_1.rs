//! Authentication regressions.
use super::*;

#[test]
fn usage_reads_the_agents_login_and_help_lists_the_commands() {
    let mut app = app("idle", true);
    typed(&mut app, "/usage");
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    let reads: Vec<String> = app
        .commands
        .drain(..)
        .filter_map(|command| match command {
            Command::Usage { auth } => Some(auth),
            Command::Write { .. } | Command::History { .. } => None,
        })
        .collect();
    assert_eq!(reads, ["main"]);
    assert_eq!(
        (app.status.as_str(), app.input.as_str()),
        ("reading main usage…", "")
    );
    let now = crate::state::now();
    app.update(Update::Usage {
        auth: String::from("main"),
        result: Ok(json!([{"auth": "main", "observed_at": now,
            "windows": [{"name": "five_hour", "used_percent": 12.0}]}])),
    });
    let last = |app: &App| match app.model.as_ref().unwrap().entries().last() {
        Some(Entry::Notice(text)) => text.clone(),
        other => panic!("expected a notice, found {other:?}"),
    };
    assert_eq!(last(&app), "main: 5h 12% used · reported just now");
    assert!(!app.status.starts_with("reading"));
    app.update(Update::Usage {
        auth: String::from("main"),
        result: Err(String::from("daemon_unavailable: retry")),
    });
    assert_eq!(app.status, "usage failed: daemon_unavailable: retry");
    typed(&mut app, "/help");
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    let help = last(&app);
    assert!(
        help.starts_with("view commands: /name /model /effort"),
        "{help}"
    );
    assert!(help.ends_with("//text sends /text"), "{help}");
    assert!(app.commands.is_empty());
    // Before the first snapshot the login is unknown.
    let mut early = App::new(String::from("a"), Mode::default());
    typed(&mut early, "/usage");
    press(&mut early, KeyCode::Enter, KeyModifiers::NONE);
    assert!(early.commands.is_empty());
    assert_eq!(early.status, "usage: the agent's login is not known yet");
}
