use super::*;
use ratatui::crossterm::event::KeyModifiers;
use serde_json::json;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn form() -> Card {
    Card::from_detail(&json!({
        "id": "ap-1", "state": "pending", "title": "Deploy", "message": "Pick settings",
        "destructive": true,
        "schema": {"type": "object", "required": ["region"], "properties": {
            "replicas": {"type": "integer", "minimum": 1, "maximum": 3},
            "region": {"type": "string", "enum": ["eu", "us"]},
            "dry_run": {"type": "boolean"},
        }},
    }))
    .unwrap()
}

#[test]
fn closed_or_missing_approvals_have_no_card() {
    assert!(Card::from_detail(&Value::Null).is_none());
    assert!(Card::from_detail(&json!({"id": "a", "state": "accepted"})).is_none());
    let simple =
        Card::from_detail(&json!({"id": "a", "state": "pending", "message": "Go?"})).unwrap();
    assert_eq!(simple.content(), Ok(None));
    assert!(simple.simple());
}

#[test]
fn array_answers_use_json_types_and_keep_schema_bounds() {
    let detail = json!({"id":"array", "state":"pending", "message":"Select targets",
        "schema":{"type":"object", "properties":{
            "targets":{"type":"array", "items":{"type":"string", "enum":["a","b"]}, "minItems":1, "maxItems":2},
            "optional":{"type":"array", "items":{"type":"string", "enum":["x"]}}
        }, "required":["targets"]}});
    let mut card = Card::from_detail(&detail).unwrap();
    assert!(card.content().is_err());
    card.paste("[\"a\"]");
    assert_eq!(card.content().unwrap(), Some(json!({"targets":["a"]})));
    for invalid in [
        "[]",
        "[\"other\"]",
        "[\"a\",\"b\",\"a\"]",
        "\"a\"",
        "[true]",
        "[",
    ] {
        card.fields[0].value = invalid.into();
        assert!(card.content().is_err(), "{invalid}");
    }
    card.fields[0].value = "[\"a\",\"b\"]".into();
    card.key(key(KeyCode::Down));
    card.key(key(KeyCode::Char('[')));
    card.key(key(KeyCode::Char(']')));
    assert_eq!(
        card.content().unwrap(),
        Some(json!({"targets":["a","b"], "optional":[]}))
    );
}

#[test]
fn array_editor_shows_the_allowed_items() {
    let card = Card::from_detail(&json!({"id":"array", "schema":{"type":"object",
        "properties":{"targets":{"type":"array", "items":{"type":"string", "enum":["west","east"]}}}}})).unwrap();
    let text = card
        .lines(120, 9)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("JSON array"));
    assert!(text.contains("west") && text.contains("east"));
}

#[test]
fn forms_put_required_fields_first_and_validate_with_the_schema() {
    let mut card = form();
    let names: Vec<_> = card
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect();
    assert_eq!(names[0], "region");
    assert_eq!(card.content(), Err(String::from("region is required")));
    card.key(key(KeyCode::Right));
    assert_eq!(card.fields[0].value, "eu");
    card.key(key(KeyCode::Left));
    assert_eq!(card.fields[0].value, "us");
    // Move to the integer field and type an out-of-range value.
    let replicas = card
        .fields
        .iter()
        .position(|field| field.name == "replicas")
        .unwrap();
    card.selected = replicas;
    assert!(!card.simple());
    card.key(key(KeyCode::Char('9')));
    assert_eq!(
        card.content(),
        Err(String::from(
            "the answer does not match the question's schema"
        ))
    );
    card.key(key(KeyCode::Backspace));
    card.key(key(KeyCode::Char('2')));
    let dry_run = card
        .fields
        .iter()
        .position(|field| field.name == "dry_run")
        .unwrap();
    card.selected = dry_run;
    card.key(key(KeyCode::Right));
    assert_eq!(
        card.content(),
        Ok(Some(
            json!({"region": "us", "replicas": 2, "dry_run": true})
        ))
    );
    let text: Vec<String> = card.lines(80, 50).iter().map(ToString::to_string).collect();
    assert_eq!(text[0], "Deploy");
    assert!(
        text.iter().any(|line| line.contains("region*: us")),
        "{text:?}"
    );
}

#[test]
fn buttons_answer_by_column_and_field_clicks_select_and_cycle() {
    let mut card = Card::from_detail(&json!({
        "id": "q1",
        "title": "Deploy",
        "message": "Pick settings.\nThen accept.",
        "schema": {
            "type": "object",
            "properties": {
                "region": {"type": "string", "enum": ["eu", "us"]},
                "dry_run": {"type": "boolean"},
                "note": {"type": "string"}
            },
            "required": ["region"]
        }
    }))
    .unwrap();
    let (row, ranges) = card.buttons();
    assert!(
        row.to_string().starts_with(" Accept   Decline   Cancel "),
        "{row}"
    );
    assert_eq!(
        ranges,
        [(0, 8, "accept"), (9, 18, "decline"), (19, 27, "cancel")]
    );
    assert_eq!(
        (
            card.button_at(0),
            card.button_at(8),
            card.button_at(12),
            card.button_at(26)
        ),
        (Some("accept"), None, Some("decline"), Some("cancel"))
    );
    assert_eq!(card.button_at(40), None);
    // Lines: title, two message lines, then the fields, required first.
    let lines: Vec<String> = card.lines(80, 50).iter().map(ToString::to_string).collect();
    assert!(lines[3].contains("region*:"), "{lines:?}");
    let dry_run = lines
        .iter()
        .position(|line| line.contains("dry_run"))
        .unwrap();
    // A click on the field's name selects it; on its value it cycles.
    card.click(dry_run, 2, 80, 50);
    assert_eq!(card.fields[card.selected].name, "dry_run");
    assert_eq!(card.fields[card.selected].value, "");
    card.click(dry_run, 30, 80, 50);
    assert_eq!(card.fields[card.selected].value, "true");
    card.click(3, 30, 80, 50);
    assert_eq!(card.fields[card.selected].value, "eu");
    card.click(3, 30, 80, 50);
    assert_eq!(card.fields[card.selected].value, "us");
    // A text field is only selected; title and message lines do nothing.
    let note = lines.iter().position(|line| line.contains("note")).unwrap();
    card.click(note, 30, 80, 50);
    assert_eq!(
        (
            card.fields[card.selected].name.as_str(),
            card.fields[card.selected].value.as_str()
        ),
        ("note", "")
    );
    card.click(0, 0, 80, 50);
    card.click(1, 0, 80, 50);
    assert_eq!(card.fields[card.selected].name, "note");
}

#[test]
fn long_lines_wrap_to_the_card_width_and_clicks_follow_the_wrapped_rows() {
    let mut card = Card::from_detail(&json!({
        "id": "q1",
        "title": "Finish changelog",
        "message": "Tests pass; the review found one bug and it is fixed. Write the CHANGELOG entry and finish?",
        "schema": {
            "type": "object",
            "properties": {"dry_run": {"type": "boolean"}}
        }
    }))
    .unwrap();
    let lines: Vec<String> = card.lines(30, 50).iter().map(ToString::to_string).collect();
    // Nothing is cut at the edge: every word of the message survives.
    assert!(lines.iter().all(|line| line.width() <= 30), "{lines:?}");
    assert!(lines.len() > 3, "{lines:?}");
    let text = lines
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        text.contains("Write the CHANGELOG entry and finish?"),
        "{text}"
    );
    let field = lines
        .iter()
        .position(|line| line.contains("dry_run"))
        .unwrap();
    assert_eq!(field, lines.len() - 1);
    // A click on a message row does nothing; the field's row cycles its value.
    card.click(field - 1, 30, 30, 50);
    assert_eq!(card.fields[0].value, "");
    card.click(field, 30, 30, 50);
    assert_eq!(card.fields[0].value, "true");
    // A wrapped value row still belongs to its field.
    let narrow = card.lines(12, 50);
    let first = narrow
        .iter()
        .position(|line| line.to_string().contains("dry_run"))
        .unwrap();
    assert!(narrow.len() > first + 1, "{narrow:?}");
    card.click(first + 1, 0, 12, 50);
    assert_eq!(card.fields[0].value, "false");
}

#[test]
fn control_chords_are_not_typed_into_form_fields() {
    let mut card = Card::from_detail(&json!({
        "id": "q1",
        "schema": {"type": "object", "properties": {"note": {"type": "string"}}}
    }))
    .unwrap();
    card.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    card.key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT));
    card.key(key(KeyCode::Char('b')));
    assert_eq!(card.fields[0].value, "b");
}

#[test]
fn a_card_taller_than_its_rows_keeps_the_title_the_end_and_the_selected_field() {
    let message = "word ".repeat(60);
    let mut card = Card::from_detail(&json!({
        "id": "q1",
        "title": "Deploy",
        "message": format!("{message}\nShip it?"),
        "schema": {
            "type": "object",
            "properties": {
                "region": {"type": "string", "enum": ["eu", "us"]},
                "note": {"type": "string"}
            },
            "required": ["region"]
        }
    }))
    .unwrap();
    let text = |card: &Card, rows| -> Vec<String> {
        card.lines(30, rows)
            .iter()
            .map(ToString::to_string)
            .collect()
    };
    assert!(card.lines(30, 100).len() > 9);
    // Nine rows: the title, a mark for the skipped text, then the end.
    let lines = text(&card, 9);
    assert_eq!(lines.len(), 9);
    assert_eq!(lines[0], "Deploy");
    assert_eq!(lines[1], "…");
    assert!(lines.iter().any(|line| line == "Ship it?"), "{lines:?}");
    let region = lines
        .iter()
        .position(|line| line.contains("region*"))
        .unwrap();
    let note = lines.iter().position(|line| line.contains("note")).unwrap();
    // Clicks follow the shown rows: the mark and the title do nothing.
    card.click(1, 0, 30, 9);
    card.click(0, 0, 30, 9);
    assert_eq!(card.selected, 0);
    card.click(note, 0, 30, 9);
    assert_eq!(card.fields[card.selected].name, "note");
    card.click(region, 20, 30, 9);
    assert_eq!(
        (card.fields[0].name.as_str(), card.fields[0].value.as_str()),
        ("region", "eu")
    );
    // Too few rows for marks: the selected field stays.
    card.selected = 0;
    let lines = text(&card, 3);
    assert_eq!(lines.len(), 3);
    assert!(
        lines.iter().any(|line| line.contains("region*")),
        "{lines:?}"
    );
    // A short card shows everything from the top.
    let short = Card::from_detail(&json!({"id": "q2", "message": "Go?"})).unwrap();
    assert_eq!(short.lines(30, 9).len(), 2);
}

#[test]
fn a_selected_field_above_the_end_shows_with_marks_for_the_hidden_rows() {
    let properties: serde_json::Map<String, Value> = (0..30)
        .map(|index| (format!("f{index:02}"), json!({"type": "string"})))
        .collect();
    let mut card = Card::from_detail(&json!({
        "id": "q1",
        "title": "Settings",
        "message": "Fill these in.",
        "schema": {"type": "object", "properties": properties}
    }))
    .unwrap();
    let text = |card: &Card| -> Vec<String> {
        card.lines(40, 12).iter().map(ToString::to_string).collect()
    };
    // The first field is near the top: the top shows, and a mark ends it.
    let lines = text(&card);
    assert_eq!(lines.len(), 12);
    assert_eq!(lines[0], "Settings");
    assert!(lines[2].contains("f00"), "{lines:?}");
    assert_eq!(lines[11], "…");
    // A middle field: marks above and below it.
    card.selected = 15;
    let lines = text(&card);
    assert_eq!((lines[1].as_str(), lines[11].as_str()), ("…", "…"));
    assert!(lines[2].contains("▸ f15"), "{lines:?}");
    // Clicks on the marks do nothing; a field row selects that field.
    card.click(11, 0, 40, 12);
    card.click(1, 0, 40, 12);
    assert_eq!(card.selected, 15);
    card.click(3, 0, 40, 12);
    assert_eq!(card.selected, 16);
    // The last fields show the end, with one mark above.
    card.selected = 29;
    let lines = text(&card);
    assert_eq!(lines[1], "…");
    assert!(lines[11].contains("▸ f29"), "{lines:?}");
    assert!(lines[2..].iter().all(|line| line != "…"), "{lines:?}");
}
