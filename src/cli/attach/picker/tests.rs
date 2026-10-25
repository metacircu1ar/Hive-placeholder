use super::*;

fn names(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("model-{index:02}"))
        .collect()
}

fn text(picker: &Picker, width: usize, rows: usize) -> Vec<String> {
    picker
        .lines(width, rows)
        .iter()
        .map(ToString::to_string)
        .collect()
}

#[test]
fn a_picker_needs_names_and_starts_on_the_current_one() {
    assert!(Picker::new(Kind::Model, "codex", &[], "m").is_none());
    let listed = names(3);
    let picker = Picker::new(Kind::Model, "codex", &listed, "model-01").unwrap();
    assert_eq!(picker.chosen(), "model-01");
    assert_eq!(picker.title(), "model (codex)");
    assert_eq!(
        text(&picker, 40, 9),
        ["   model-00", "▸✓ model-01", "   model-02"]
    );
    // An unlisted current value starts on the first name, and none is marked.
    let picker = Picker::new(Kind::Effort, "claude", &listed, "other").unwrap();
    assert_eq!(picker.chosen(), "model-00");
    assert_eq!(picker.title(), "effort (claude)");
    assert!(text(&picker, 40, 9).iter().all(|line| !line.contains('✓')));
    assert!(picker.footer().to_string().contains("/effort NAME"));
}

#[test]
fn moves_stop_at_either_end() {
    let listed = names(3);
    let mut picker = Picker::new(Kind::Model, "codex", &listed, "model-00").unwrap();
    picker.up();
    assert_eq!(picker.chosen(), "model-00");
    for _ in 0..5 {
        picker.down();
    }
    assert_eq!(picker.chosen(), "model-02");
    picker.select(9);
    assert_eq!(picker.chosen(), "model-02");
}

#[test]
fn long_lists_scroll_with_the_selection_and_rows_map_to_names() {
    let listed = names(MAX_LISTED);
    let mut picker = Picker::new(Kind::Model, "codex", &listed, "model-00").unwrap();
    assert_eq!(text(&picker, 40, 9).len(), 9);
    assert_eq!(picker.at(0, 9), Some(0));
    assert_eq!(picker.at(8, 9), Some(8));
    assert_eq!(picker.at(9, 9), None);
    for _ in 0..12 {
        picker.down();
    }
    // The selection (12) is the last row shown: names 4 to 12.
    let lines = text(&picker, 40, 9);
    assert_eq!(lines[0], "   model-04");
    assert_eq!(lines[8], "▸  model-12");
    assert_eq!(picker.at(0, 9), Some(4));
    assert_eq!(picker.at(8, 9), Some(12));
    // The last name selected is the last row, in full.
    picker.select(MAX_LISTED - 1);
    let lines = text(&picker, 40, 9);
    assert_eq!(lines.len(), 9);
    assert_eq!(lines[8], format!("▸  model-{:02}", MAX_LISTED - 1));
    // No room still shows the selected name.
    assert_eq!(text(&picker, 40, 0).len(), 1);
    assert_eq!(picker.at(0, 0), Some(MAX_LISTED - 1));
    // Narrow cards cut names instead of wrapping them; the marks come first.
    let narrow = text(&picker, 4, 9);
    assert!(
        narrow.iter().all(|line| line.chars().count() <= 4),
        "{narrow:?}"
    );
    let current = Picker::new(Kind::Model, "codex", &listed, "model-00").unwrap();
    assert_eq!(text(&current, 2, 9)[0], "▸✓");
}
