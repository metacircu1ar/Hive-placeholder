use super::*;

#[test]
fn titles_are_set_only_for_agent_ids_and_as_one_osc_sequence() {
    let id = "11111111-2222-4333-8444-555555555555";
    assert_eq!(title(true, id), Some(format!("hive:{id}")));
    assert_eq!(title(false, id), None);
    // Free text never reaches the terminal title.
    assert_eq!(title(true, "agent\u{7};\u{1b}]0;evil"), None);
    let mut bytes = Vec::new();
    ratatui::crossterm::queue!(
        bytes,
        ratatui::crossterm::terminal::SetTitle(title(true, id).unwrap())
    )
    .unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(
        text == format!("\u{1b}]0;hive:{id}\u{7}") || text == format!("\u{1b}]2;hive:{id}\u{7}"),
        "{text:?}"
    );
}

#[test]
fn view_titles_use_the_resolved_agent_instead_of_the_requested_target() {
    let resolved = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    let view = View::detached(
        "short-target",
        Mode::default(),
        &serde_json::json!({
            "agent":{"id":resolved}, "events":{"events":[]}
        }),
    );
    assert_eq!(
        title(true, view.agent_id().unwrap()),
        Some(format!("hive:{resolved}"))
    );
}

#[test]
fn clipboard_copies_are_one_osc_52_sequence_with_base64_text() {
    assert_eq!(osc52("abc"), "\x1b]52;c;YWJj\x07");
    let id = "00000000-0000-4000-8000-000000000001";
    let mut out = Vec::new();
    copy_to_clipboard(&mut out, id).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.starts_with("\x1b]52;c;") && text.ends_with('\x07'),
        "{text:?}"
    );
    // The payload is base64: no control bytes reach the terminal from the text.
    assert!(!text[7..text.len() - 1].chars().any(char::is_control));
}
