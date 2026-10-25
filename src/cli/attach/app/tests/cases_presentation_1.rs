//! Presentation regressions.
use super::*;

#[test]
fn read_only_views_never_write() {
    let mut app = App::new(
        String::from("a"),
        Mode {
            read_only: true,
            ..Mode::default()
        },
    );
    app.update(snapshot("idle", true));
    typed(&mut app, "hello");
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.commands.is_empty() && app.input.is_empty());
    assert!(
        app.screen(true, Rect::new(0, 0, 80, 24))
            .unwrap()
            .input
            .is_none()
    );
}

#[test]
fn approvals_answer_from_the_card_and_forms_validate_first() {
    let mut app = app("idle", false);
    app.update(Update::Approval(
        json!({"id": "q", "state": "pending", "message": "Go?"}),
    ));
    press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
    assert_eq!(
        writes(&mut app)[0].2,
        json!({"op": "respond", "approval_id": "q", "action": "accept"})
    );
    app.update(Update::Approval(
        json!({"id": "f", "state": "pending", "message": "Region?",
        "schema": {"type": "object", "required": ["region"],
                   "properties": {"region": {"type": "string", "enum": ["eu", "us"]}}}}),
    ));
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(writes(&mut app).is_empty() && app.status.contains("region is required"));
    press(&mut app, KeyCode::Right, KeyModifiers::NONE);
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(writes(&mut app)[0].2["content"], json!({"region": "eu"}));
    // From the input, /decline answers the same question.
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    typed(&mut app, "/decline");
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(writes(&mut app)[0].2["action"], json!("decline"));
    // Once answered, the card closes and focus returns to the input.
    app.update(Update::Approval(Value::Null));
    assert!(
        app.screen(true, Rect::new(0, 0, 80, 24))
            .unwrap()
            .card
            .is_none()
    );
}

#[test]
fn a_refused_first_attach_ends_the_view_with_the_daemon_error() {
    let mut app = App::new(String::from("missing"), Mode::default());
    let reply = json!({"error": {"code": "unknown_agent", "message": "Unknown agent."}});
    app.update(Update::Refused(reply.clone()));
    assert_eq!(app.exit, Some(Exit::Refused(reply)));
    // Before a snapshot, an unreachable daemon is shown on the connecting screen.
    let mut app = App::new(String::from("a"), Mode::default());
    app.update(Update::Disconnected);
    assert!(
        app.status.contains("not answering") && app.screen(true, Rect::new(0, 0, 80, 24)).is_none()
    );
}

#[test]
fn form_cards_ignore_single_answer_keys_and_take_pasted_text() {
    let mut app = app("idle", false);
    app.update(Update::Approval(json!({"id": "f", "state": "pending", "message": "Name?",
        "schema": {"type": "object", "required": ["name"],
                   "properties": {"name": {"type": "string"}, "mode": {"type": "string", "enum": ["a", "b"]}}}})));
    press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    app.event(&Event::Paste(String::from("release")));
    // Typed letters go to the text field rather than answering the form.
    press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
    press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
    assert!(writes(&mut app).is_empty());
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(
        writes(&mut app)[0].2["content"],
        json!({"name": "released"})
    );
}

#[test]
fn scrolling_down_after_the_top_is_reached_moves_at_once() {
    let mut app = app("idle", false);
    app.shown = Shown {
        lines: 100,
        height: 10,
        more: true,
        ..Shown::default()
    };
    for _ in 0..12 {
        press(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
    }
    // The next frame rendered everything: 100 lines, so the top is at 90.
    app.shown.more = false;
    press(&mut app, KeyCode::PageDown, KeyModifiers::NONE);
    assert_eq!(app.scroll, 80);
}

#[test]
fn hosts_set_the_hint_track_writes_and_focus_the_next_card() {
    let mut app = App::new(String::from("a"), Mode::default());
    app.set_hint(String::from("custom keys"));
    app.update(snapshot("idle", false));
    assert_eq!(app.status, "custom keys");
    app.set_hint(String::from("newer keys"));
    assert_eq!(app.status, "newer keys");
    assert!(!app.busy());
    typed(&mut app, "reply: x");
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.busy());
    let action = writes(&mut app)[0].0;
    app.update(Update::Done {
        action,
        label: String::from("send"),
        result: Ok(json!({})),
    });
    assert!(!app.busy());
    // Asked before the card exists, focus moves once it arrives.
    app.focus_card();
    assert_eq!(app.focus, Focus::Input);
    app.update(Update::Approval(json!({"id": "q1", "kind": "approval",
        "request": {"message": "Deploy?"}})));
    assert_eq!(app.focus, Focus::Card);
    app.focus = Focus::Input;
    app.update(Update::Approval(json!({"id": "q2", "kind": "approval",
        "request": {"message": "Again?"}})));
    assert_eq!(app.focus, Focus::Input);
    app.focus_card();
    assert_eq!(app.focus, Focus::Card);
}

#[test]
fn clicks_focus_the_part_under_them_and_the_wheel_scrolls() {
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    let mut app = app("idle", false);
    app.update(Update::Approval(json!({"id": "q1", "message": "Deploy?"})));
    let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
    terminal
        .draw(|frame| app.draw_into(frame, Rect::new(0, 0, 60, 24), true))
        .unwrap();
    let regions = app.shown.regions;
    let inside = |area: Rect| (area.x + 1, area.y + 1);
    let left = MouseEventKind::Down(MouseButton::Left);
    assert!(mouse_at(&mut app, left, inside(regions.card)));
    assert_eq!(app.focus, Focus::Card);
    assert!(mouse_at(&mut app, left, inside(regions.transcript)));
    assert_eq!(app.focus, Focus::Transcript);
    assert!(mouse_at(&mut app, left, inside(regions.input)));
    assert_eq!(app.focus, Focus::Input);
    // The wheel scrolls three lines a step; scrolling past the top asks for
    // older history, as PgUp does.
    app.shown.more = true;
    assert!(mouse_at(
        &mut app,
        MouseEventKind::ScrollUp,
        inside(regions.transcript)
    ));
    assert_eq!(app.scroll, 3);
    assert!(mouse_at(
        &mut app,
        MouseEventKind::ScrollDown,
        inside(regions.transcript)
    ));
    assert_eq!(app.scroll, 0);
    // Outside the view, and for releases or moves, nothing changes.
    assert!(!mouse_at(&mut app, left, (70, 30)));
    assert!(!mouse_at(
        &mut app,
        MouseEventKind::Moved,
        inside(regions.input)
    ));
    assert!(!mouse_at(
        &mut app,
        MouseEventKind::Up(MouseButton::Left),
        inside(regions.input)
    ));
    // A click disarms the Ctrl-C confirmation, as a key does.
    app.quit_armed = true;
    assert!(mouse_at(&mut app, left, inside(regions.transcript)));
    assert!(!app.quit_armed);
}

#[test]
fn card_buttons_answer_when_clicked() {
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    let left = MouseEventKind::Down(MouseButton::Left);
    for (label, action) in [
        (" Accept ", "accept"),
        (" Decline ", "decline"),
        (" Cancel ", "cancel"),
    ] {
        let mut app = app("idle", false);
        app.update(Update::Approval(json!({"id": "q1", "message": "Deploy?"})));
        let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
        terminal
            .draw(|frame| app.draw_into(frame, Rect::new(0, 0, 60, 24), true))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let at = (0..24)
            .find_map(|y| {
                let row: String = (0..60)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect();
                row.find(label)
                    .map(|byte| (u16::try_from(row[..byte].chars().count()).unwrap() + 1, y))
            })
            .unwrap();
        assert!(mouse_at(&mut app, left, at));
        let written = writes(&mut app);
        assert_eq!(written.len(), 1, "{label}");
        assert_eq!(
            (&written[0].2["op"], &written[0].2["action"]),
            (&json!("respond"), &json!(action))
        );
    }
}

#[test]
fn control_chords_never_answer_or_type_into_a_card() {
    let mut app = app("idle", false);
    app.update(Update::Approval(json!({"id": "q1", "message": "Deploy?"})));
    app.focus = Focus::Card;
    // The TUI's prefix sent twice reaches the view as Ctrl-A.
    for code in ['a', 'd', 'c'] {
        press(&mut app, KeyCode::Char(code), KeyModifiers::CONTROL);
        press(&mut app, KeyCode::Char(code), KeyModifiers::ALT);
    }
    assert!(writes(&mut app).is_empty());
    press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
    assert_eq!(writes(&mut app)[0].2["action"], "accept");
}

#[test]
fn a_click_sets_the_name_under_it_in_a_scrolled_picker() {
    use ratatui::{Terminal, backend::TestBackend};
    let names: Vec<String> = (0..20).map(|index| format!("model-{index:02}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut app = listed_app(&refs);
    run(&mut app, "/model");
    for _ in 0..15 {
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    }
    let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
    terminal
        .draw(|frame| app.draw_into(frame, Rect::new(0, 0, 60, 24), true))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let row = |label: &str| {
        (0..24).find(|y| {
            (0..60)
                .map(|x| buffer[(x, *y)].symbol().to_owned())
                .collect::<String>()
                .contains(label)
        })
    };
    // Twelve text rows: names 4 to 15 under the frame title.
    assert!(row("model (codex)").is_some());
    assert!(row("model-03").is_none() && row("model-04").is_some() && row("model-15").is_some());
    let left = MouseEventKind::Down(MouseButton::Left);
    // The footer row and the border pick nothing.
    let footer = row("Enter set").unwrap();
    assert!(mouse_at(&mut app, left, (4, footer)));
    assert!(mouse_at(&mut app, left, (4, footer + 1)));
    assert!(writes(&mut app).is_empty());
    assert!(mouse_at(&mut app, left, (4, row("model-07").unwrap())));
    assert_eq!(
        writes(&mut app)[0].2["options"],
        json!({"model": "model-07"})
    );
    assert!(card_text(&app).is_none());
}

#[test]
fn footer_rows_esc_in_the_input_and_attention_jumps_close_or_ignore_the_picker() {
    use ratatui::{Terminal, backend::TestBackend};
    let names: Vec<String> = (0..20).map(|index| format!("model-{index:02}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    // A short view squeezes the card: its footer covers a name row.
    let mut app = listed_app(&refs);
    run(&mut app, "/model");
    let mut terminal = Terminal::new(TestBackend::new(60, 14)).unwrap();
    terminal
        .draw(|frame| app.draw_into(frame, Rect::new(0, 0, 60, 14), true))
        .unwrap();
    let card = app.shown.regions.card;
    // Twelve rows were asked for: nine names, the footer and the border.
    assert!(card.height < 12, "{card:?}");
    let footer = card.y + card.height - 2;
    let left = MouseEventKind::Down(MouseButton::Left);
    assert!(mouse_at(&mut app, left, (4, footer)));
    assert!(writes(&mut app).is_empty());
    assert!(card_text(&app).is_some());
    // With a question under the picker and the input focused, Esc closes the
    // picker and a second Esc does not interrupt.
    app.update(Update::Approval(json!({"id": "q", "message": "Go?"})));
    app.focus = Focus::Input;
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(card_text(&app).unwrap().0, "question");
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(writes(&mut app).is_empty());
    // An attention jump asked for before the question arrives closes a
    // picker opened meanwhile.
    let mut app = listed_app(&refs);
    app.focus_card();
    run(&mut app, "/model");
    app.update(Update::Approval(json!({"id": "r", "message": "Ship?"})));
    assert_eq!(app.focus, Focus::Card);
    assert_eq!(card_text(&app).unwrap().0, "question");
}

#[test]
fn id_shows_the_agents_id_and_hands_it_to_the_clipboard_once() {
    let mut app = app("idle", false);
    run(&mut app, "/id");
    assert_eq!(app.status, "id: a (copy sent to the terminal clipboard)");
    assert_eq!(app.take_clipboard().as_deref(), Some("a"));
    assert_eq!(app.take_clipboard(), None);
    assert!(app.input.is_empty() && writes(&mut app).is_empty());
    // Arguments are refused with the command's usage, like other commands.
    run(&mut app, "/id now");
    assert_eq!(app.status, "usage: /id");
    // Before the first snapshot the view's target stands in for the ID.
    let mut app = App::new(String::from("target-1"), Mode::default());
    run(&mut app, "/id");
    assert_eq!(app.take_clipboard().as_deref(), Some("target-1"));
}
