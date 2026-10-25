//! Indexing regressions.
use super::*;

#[test]
fn model_and_effort_without_an_argument_pick_from_the_listed_names() {
    let mut app = listed_app(&["gpt-6", "m", "gpt-6-luna"]);
    run(&mut app, "/model");
    assert_eq!(app.focus, Focus::Card);
    assert!(app.input.is_empty());
    let (title, text) = card_text(&app).unwrap();
    assert_eq!(title, "model (codex)");
    // It starts on the agent's current model.
    assert_eq!(text, ["   gpt-6", "▸✓ m", "   gpt-6-luna"]);
    press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(
        writes(&mut app)[0].2,
        json!({"op": "set_options", "target": "a", "options": {"model": "gpt-6-luna"}})
    );
    assert_eq!(app.focus, Focus::Input);
    assert!(card_text(&app).is_none());
    run(&mut app, "/effort");
    assert_eq!(card_text(&app).unwrap().0, "effort (codex)");
    // Chords such as the TUI's prefix never move or pick.
    press(&mut app, KeyCode::Down, KeyModifiers::CONTROL);
    press(&mut app, KeyCode::Enter, KeyModifiers::ALT);
    assert!(writes(&mut app).is_empty());
    press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(writes(&mut app)[0].2["options"], json!({"effort": "high"}));
    // Esc closes without a write; a typed name still sets any value.
    run(&mut app, "/effort");
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(writes(&mut app).is_empty() && card_text(&app).is_none());
    assert_eq!(app.focus, Focus::Input);
    run(&mut app, "/model custom-1");
    assert_eq!(
        writes(&mut app)[0].2["options"],
        json!({"model": "custom-1"})
    );
}
