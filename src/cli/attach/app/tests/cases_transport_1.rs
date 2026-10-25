//! Transport regressions.
use super::*;

#[test]
fn only_the_active_view_of_a_split_places_the_cursor() {
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    let mut left = app("idle", false);
    let mut right = app("idle", false);
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    let (first, second) = (Rect::new(0, 0, 40, 20), Rect::new(40, 0, 40, 20));
    for active_right in [true, false] {
        terminal
            .draw(|frame| {
                left.draw_into(frame, first, !active_right);
                right.draw_into(frame, second, active_right);
            })
            .unwrap();
        let backend = terminal.backend();
        assert!(backend.cursor_visible());
        let x = backend.cursor_position().x;
        assert_eq!(x >= 40, active_right, "cursor at {x}");
    }
    // No active view: no cursor at all.
    terminal
        .draw(|frame| {
            left.draw_into(frame, first, false);
            right.draw_into(frame, second, false);
        })
        .unwrap();
    assert!(!terminal.backend().cursor_visible());
    // Before the snapshot the view says what it waits for.
    let mut connecting = App::new(String::from("agent-9"), Mode::default());
    connecting.update(Update::Waiting(true));
    terminal
        .draw(|frame| connecting.draw_into(frame, first, true))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let top: String = (0..40)
        .map(|x| buffer[(x, 0)].symbol().to_owned())
        .collect();
    assert!(
        top.starts_with("Connecting to agent-9: waiting for a"),
        "{top}"
    );
}

#[test]
fn capacity_waits_show_and_clear_in_the_status() {
    let mut app = App::new(String::from("a"), Mode::default());
    app.update(Update::Waiting(true));
    assert!(app.waiting() && app.status == WAITING);
    app.update(snapshot("idle", false));
    app.update(Update::Waiting(false));
    assert!(!app.waiting());
    assert!(app.status.starts_with("Enter send"), "{}", app.status);
    // A cleared wait does not replace a newer message.
    app.update(Update::Waiting(true));
    app.update(Update::Problem(String::from("other")));
    app.update(Update::Waiting(false));
    assert_eq!(app.status, "other");
}

#[test]
fn the_picker_covers_a_waiting_question_until_it_closes() {
    let mut app = listed_app(&["gpt-6", "m"]);
    app.update(Update::Approval(json!({"id": "q", "message": "Go?"})));
    run(&mut app, "/model");
    assert_eq!(card_text(&app).unwrap().0, "model (codex)");
    assert!(
        app.status.contains("a question is waiting"),
        "{}",
        app.status
    );
    // Answer keys and pastes do nothing while the picker has the card's place.
    press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
    app.event(&Event::Paste(String::from("eu")));
    assert!(writes(&mut app).is_empty());
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    let (title, text) = card_text(&app).unwrap();
    assert_eq!(title, "question");
    // The note about the covered question goes with the picker.
    assert!(
        !app.status.contains("a question is waiting"),
        "{}",
        app.status
    );
    assert!(text.iter().any(|line| line == "Go?"), "{text:?}");
    press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(app.focus, Focus::Card);
    press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
    assert_eq!(writes(&mut app)[0].2["action"], json!("accept"));
    // A question answered elsewhere leaves an open picker in place.
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    run(&mut app, "/model");
    app.update(Update::Approval(json!({"id": "q2", "message": "Again?"})));
    assert!(
        app.status.contains("a question is waiting"),
        "{}",
        app.status
    );
    app.update(Update::Approval(Value::Null));
    assert_eq!(app.focus, Focus::Card);
    assert_eq!(card_text(&app).unwrap().0, "model (codex)");
    assert!(
        !app.status.contains("a question is waiting"),
        "{}",
        app.status
    );
    // A question arriving under the picker is announced; the attention
    // list's jump to it closes the picker.
    app.status.clear();
    app.update(Update::Approval(json!({"id": "r", "message": "Ship?"})));
    assert_eq!(card_text(&app).unwrap().0, "model (codex)");
    assert!(
        app.status.contains("a question is waiting"),
        "{}",
        app.status
    );
    app.focus_card();
    assert_eq!(app.focus, Focus::Card);
    assert_eq!(card_text(&app).unwrap().0, "question");
    assert!(
        !app.status.contains("a question is waiting"),
        "{}",
        app.status
    );
}
