use super::*;

fn press(dialog: &mut Dialog, code: KeyCode) -> Key {
    dialog.key(&KeyEvent::new(code, KeyModifiers::NONE))
}

fn loaded() -> Dialog {
    let mut dialog = Dialog::new(String::from("/work"), false);
    dialog.roles(&json!({
        "default_provider": null,
        "roles": [{"name": "implementor", "provider": "codex"}, {"name": "reviewer", "provider": "claude"}],
        "profiles": [{"name": "main", "provider": "codex"}],
        "groups": [{"name": "plans", "provider": "codex"}],
    }));
    dialog
}

#[test]
fn sandbox_selector_cycles_defaults_and_all_three_explicit_modes() {
    let mut dialog = loaded();
    dialog.body = "work".into();
    assert!(dialog.request().unwrap().get("sandbox").is_none());
    dialog.focus = Field::Sandbox;
    for sandbox in SANDBOXES {
        press(&mut dialog, KeyCode::Right);
        assert_eq!(dialog.request().unwrap()["sandbox"], sandbox);
        assert!(text(&dialog).contains(sandbox));
    }
    press(&mut dialog, KeyCode::Right);
    assert!(dialog.request().unwrap().get("sandbox").is_none());
}

fn text(dialog: &Dialog) -> String {
    dialog
        .lines(74)
        .lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn choices_and_fields_build_one_spawn_request() {
    let mut dialog = loaded();
    assert_eq!(dialog.focus, Field::Body);
    dialog.paste("Review the change\nthen report");
    press(&mut dialog, KeyCode::Tab);
    assert_eq!(dialog.focus, Field::Role);
    press(&mut dialog, KeyCode::Right);
    press(&mut dialog, KeyCode::Right);
    assert!(text(&dialog).contains("‹ reviewer ›"));
    assert!(text(&dialog).contains("provider: claude"));
    // The name is typed; surrounding spaces are dropped.
    press(&mut dialog, KeyCode::Tab);
    assert_eq!(dialog.focus, Field::Name);
    assert!(text(&dialog).contains("(none)"));
    dialog.paste(" diff reviewer ");
    press(&mut dialog, KeyCode::Tab);
    press(&mut dialog, KeyCode::Left);
    // Login choices list profiles, then groups.
    assert!(text(&dialog).contains("‹ plans ›"));
    assert!(text(&dialog).contains("provider: codex"));
    press(&mut dialog, KeyCode::Tab);
    for character in "gpt-x".chars() {
        press(&mut dialog, KeyCode::Char(character));
    }
    press(&mut dialog, KeyCode::Tab);
    dialog.paste("low");
    let Key::Submit { id, request } = press(&mut dialog, KeyCode::Enter) else {
        panic!("no request");
    };
    assert_eq!(dialog.request.as_deref(), Some(id.as_str()));
    assert_eq!(
        request,
        json!({"op": "spawn", "role": "reviewer", "name": "diff reviewer", "auth": "plans",
               "model": "gpt-x", "effort": "low", "cwd": "/work",
               "body": "Review the change\nthen report"})
    );
    // While the spawn is in flight, keys wait for its reply.
    assert_eq!(press(&mut dialog, KeyCode::Esc), Key::None);
}

#[test]
fn local_checks_name_their_field_before_sending() {
    let mut dialog = Dialog::new(String::from("relative"), true);
    assert_eq!(press(&mut dialog, KeyCode::Enter), Key::None);
    assert_eq!(dialog.focus, Field::Body);
    assert!(text(&dialog).contains("write the agent's first task"));
    dialog.paste("work");
    assert_eq!(press(&mut dialog, KeyCode::Enter), Key::None);
    assert_eq!(dialog.focus, Field::Directory);
    assert!(text(&dialog).contains("use an absolute directory"));
    // Without a role or login, only the task and directory are sent.
    for _ in 0.."relative".len() {
        press(&mut dialog, KeyCode::Backspace);
    }
    dialog.paste("/abs");
    let Key::Submit { request, .. } = press(&mut dialog, KeyCode::Enter) else {
        panic!("no request");
    };
    assert_eq!(
        request,
        json!({"op": "spawn", "cwd": "/abs", "body": "work"})
    );
    // A dialog under --root says where the agent starts.
    dialog.roles(&json!({"roles": [], "profiles": [], "groups": []}));
    assert!(text(&dialog).contains("outside this TUI's scope"));
}

#[test]
fn daemon_errors_land_on_the_field_they_name() {
    for (error, field) in [
        (
            json!({"code": "invalid_value", "field": "/cwd", "message": "m"}),
            Some(Field::Directory),
        ),
        (
            json!({"code": "invalid_value", "field": "/body"}),
            Some(Field::Body),
        ),
        (json!({"code": "unknown_role"}), Some(Field::Role)),
        (
            json!({"code": "invalid_value", "field": "/name"}),
            Some(Field::Name),
        ),
        (json!({"code": "unknown_profile"}), Some(Field::Login)),
        (json!({"code": "unknown_model"}), Some(Field::Model)),
        (json!({"code": "model_mismatch"}), Some(Field::Model)),
        (json!({"code": "unsupported_effort"}), Some(Field::Effort)),
        (json!({"code": "effort_mismatch"}), Some(Field::Effort)),
        (json!({"code": "invalid_cwd"}), Some(Field::Directory)),
        (json!({"code": "agent_limit", "message": "Full."}), None),
    ] {
        assert_eq!(Field::of_error(&error), field, "{error}");
    }
    let mut dialog = loaded();
    dialog.request = Some(String::from("spawn-1"));
    dialog.failed(&json!({"code": "unknown_model", "message": "No such model."}));
    assert!(dialog.request.is_none());
    assert_eq!(dialog.focus, Field::Model);
    assert!(text(&dialog).contains("unknown_model: No such model."));
    dialog.failed(&json!({"code": "agent_limit", "message": "Full."}));
    assert_eq!(dialog.footer, "agent_limit: Full.");
    assert!(!text(&dialog).contains("unknown_model"));
    dialog.roles_failed("daemon_unavailable");
    assert!(dialog.footer.contains("roles unavailable"));
}

#[test]
fn rows_name_their_fields_and_the_focused_text_scrolls_into_view() {
    let mut dialog = loaded();
    press(&mut dialog, KeyCode::Tab);
    for _ in 0..6 {
        press(&mut dialog, KeyCode::Tab);
    }
    assert_eq!(dialog.focus, Field::Directory);
    let long = "/very/long/directory/".repeat(5);
    dialog.paste(&long);
    let rendered = dialog.lines(40);
    assert_eq!(rendered.lines.len(), rendered.fields.len());
    let (row, column) = rendered.cursor.unwrap();
    assert_eq!(rendered.fields[row], Some(Field::Directory));
    assert!(column < 40, "{column}");
    let shown = rendered.lines[row].to_string();
    assert!(shown.ends_with("directory/"), "{shown}");
    assert!(shown.chars().count() <= 40, "{shown}");
    // Clicks focus a field and cycle choices with ‹ and ›.
    dialog.click(Field::Login, 0);
    assert_eq!(dialog.focus, Field::Login);
    let width = "‹ (role or default) ›".chars().count();
    dialog.click(Field::Login, u16::try_from(LABEL + width - 1).unwrap());
    assert!(text(&dialog).contains("‹ main ›"));
    dialog.click(Field::Login, u16::try_from(LABEL).unwrap());
    assert!(text(&dialog).contains("‹ (role or default) ›"));
    // Clicks wait for a spawn in flight, like keys.
    dialog.request = Some(String::from("spawn-1"));
    dialog.click(Field::Model, 0);
    assert_eq!(dialog.focus, Field::Login);
}

#[test]
fn model_and_effort_cycle_through_the_providers_listed_names_or_take_typing() {
    use crate::cli::attach::{List, Lists};
    let mut dialog = loaded();
    dialog.lists = Arc::new(Lists {
        codex: List {
            models: vec![String::from("gpt-6-luna"), String::from("gpt-6")],
            efforts: vec![String::from("low")],
        },
        claude: List {
            models: vec![String::from("opus")],
            efforts: Vec::new(),
        },
    });
    dialog.paste("Do it");
    // Role none, login none: the default provider is unknown, so nothing cycles.
    dialog.focus = Field::Model;
    press(&mut dialog, KeyCode::Right);
    assert_eq!(dialog.model, "");
    assert!(!text(&dialog).contains("listed"));
    // The implementor role spawns with codex.
    dialog.focus = Field::Role;
    press(&mut dialog, KeyCode::Right);
    dialog.focus = Field::Model;
    assert!(text(&dialog).contains("←/→ 2 listed"));
    press(&mut dialog, KeyCode::Right);
    assert_eq!(dialog.model, "gpt-6-luna");
    press(&mut dialog, KeyCode::Right);
    assert_eq!(dialog.model, "gpt-6");
    // After the last name comes the default again; Left goes back.
    press(&mut dialog, KeyCode::Right);
    assert_eq!(dialog.model, "");
    press(&mut dialog, KeyCode::Left);
    assert_eq!(dialog.model, "gpt-6");
    // Typing edits the chosen name; an unlisted value cycles from the start.
    press(&mut dialog, KeyCode::Backspace);
    press(&mut dialog, KeyCode::Char('x'));
    assert_eq!(dialog.model, "gpt-x");
    press(&mut dialog, KeyCode::Right);
    assert_eq!(dialog.model, "gpt-6-luna");
    // A long typed name keeps its end and the cursor in view beside the
    // hint; with no room to type, the hint goes.
    dialog.model = "m".repeat(80);
    let rendered = dialog.lines(50);
    let row = rendered.cursor.unwrap().0;
    let line = rendered.lines[row].to_string();
    assert!(line.ends_with("  ←/→ 2 listed"), "{line}");
    assert!(line.width() <= 50, "{line}");
    assert!(rendered.cursor.unwrap().1 < 50 - "  ←/→ 2 listed".width());
    assert!(
        !dialog.lines(LABEL + 20).lines[row]
            .to_string()
            .contains("listed")
    );
    dialog.model = String::from("gpt-6");
    press(&mut dialog, KeyCode::Left);
    assert_eq!(dialog.model, "gpt-6-luna");
    press(&mut dialog, KeyCode::Tab);
    press(&mut dialog, KeyCode::Left);
    assert_eq!(dialog.effort, "low");
    let Key::Submit { request, .. } = press(&mut dialog, KeyCode::Enter) else {
        panic!("no request");
    };
    assert_eq!(
        (&request["model"], &request["effort"]),
        (&json!("gpt-6-luna"), &json!("low"))
    );
    // A claude role lists claude's names; it lists no efforts.
    let mut dialog = loaded();
    dialog.lists = Arc::new(Lists {
        claude: List {
            models: vec![String::from("opus")],
            efforts: Vec::new(),
        },
        ..Lists::default()
    });
    dialog.focus = Field::Role;
    press(&mut dialog, KeyCode::Left);
    dialog.focus = Field::Model;
    press(&mut dialog, KeyCode::Right);
    assert_eq!(dialog.model, "opus");
    dialog.focus = Field::Effort;
    press(&mut dialog, KeyCode::Right);
    assert_eq!(dialog.effort, "");
}
