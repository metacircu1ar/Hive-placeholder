use super::*;
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;

fn model() -> Model {
    let events = json!([
        {"seq": 1, "type": "message_accepted", "data": {"sender": "human", "body": "Run the tests"}},
        {"seq": 2, "type": "turn_started", "data": {"model": "gpt", "effort": "low"}},
        {"seq": 3, "type": "tool_completed", "data": {"item": {"type": "commandExecution",
            "id": "c1", "command": "make check", "exitCode": 0, "aggregatedOutput": "a\nb\nc\nd\n"}}},
        {"seq": 4, "type": "assistant_message", "data": {"item_id": "m1",
            "text": "# Result\n\nAll **green**:\n\n- one\n- `two`\n\n```sh\nok\n```"}},
        {"seq": 5, "type": "turn_finished", "data": {"status": "completed"}},
    ]);
    Model::from_snapshot(&json!({
        "agent": {"id": "agent-1", "role": "implementor", "state": "idle", "task_outcome": "open",
                  "model": "gpt", "effort": "low", "auth": "main", "profile": {"provider": "codex"},
                  "ended_at": null},
        "events": {"events": events, "cursor": 5, "pruned_through": 0, "history_expired": false},
        "queued": 0, "mail": 0, "children": 1,
    }))
}

fn render(screen: &Screen<'_>, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            draw(frame, frame.area(), screen);
        })
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

#[test]
fn frame_shows_header_transcript_input_and_keys() {
    let model = model();
    let screen = Screen {
        model: &model,
        scroll: 0,
        input: Some(("hello", 5)),
        focus: Focus::Input,
        card: None,
        status: "Enter send",
        completion: None,
        connected: true,
        active: true,
    };
    let rows = render(&screen, 60, 24);
    let text = rows.join("\n");
    assert!(rows[0].starts_with("agent-1 implementor  idle"), "{text}");
    assert!(rows[1].contains("children 1"), "{text}");
    for expected in [
        "human: Run the tests",
        "── turn: gpt low",
        "⚙ command make check  exit 0",
        "  d",
        "Result",
        "All green:",
        "• one",
        "• two",
        "  ok",
        "── completed",
        "hello",
        "Enter send",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in\n{text}");
    }
    // The collapsed tool shows only its last output lines.
    assert!(!text.contains("  a\n"), "{text}");
}

#[test]
fn full_access_badge_is_red_and_precedes_long_agent_identity() {
    let mut model = model();
    model.header.sandbox = String::from("full-access");
    model.header.name = Some("a name longer than the terminal width".repeat(4));
    let screen = Screen {
        model: &model,
        scroll: 0,
        input: None,
        focus: Focus::Input,
        card: None,
        status: "",
        completion: None,
        connected: true,
        active: true,
    };
    let header = header_lines(&model.header, &screen);
    assert_eq!(header[0].spans[0].content, "[full-access] ");
    assert_eq!(header[0].spans[0].style.fg, Some(Color::Red));
    assert!(render(&screen, 40, 12)[0].starts_with("[full-access]"));
}

#[test]
fn read_only_frames_have_no_input_and_disconnection_is_shown() {
    let model = model();
    let screen = Screen {
        model: &model,
        scroll: 0,
        input: None,
        focus: Focus::Transcript,
        card: Some(Panel {
            title: String::from("question"),
            lines: vec![Line::from("Deploy now?")],
            footer: Line::from(" Accept "),
        }),
        status: "read-only",
        completion: None,
        connected: false,
        active: true,
    };
    let text = render(&screen, 60, 24).join("\n");
    assert!(text.contains("disconnected, retrying"), "{text}");
    assert!(
        text.contains("Deploy now?") && text.contains("question") && text.contains(" Accept "),
        "{text}"
    );
}

#[test]
fn scrolling_shows_older_lines_and_stops_at_the_top() {
    let model = model();
    let at = |scroll| Screen {
        model: &model,
        scroll,
        input: None,
        focus: Focus::Transcript,
        card: None,
        status: "",
        completion: None,
        connected: true,
        active: true,
    };
    let bottom = render(&at(0), 40, 8).join("\n");
    let top = render(&at(1000), 40, 8).join("\n");
    assert!(
        bottom.contains("completed") && !bottom.contains("Run the tests"),
        "{bottom}"
    );
    assert!(top.contains("Run the tests"), "{top}");
}

#[test]
fn wrapping_breaks_words_and_long_tokens_by_display_width() {
    let lines = wrap(&[(String::from("alpha beta gamma"), Style::new())], 10);
    let text: Vec<String> = lines.iter().map(ToString::to_string).collect();
    assert_eq!(text, ["alpha beta", "gamma"]);
    let wide = wrap(&[(String::from("日本語のテキスト"), Style::new())], 6);
    assert!(wide.iter().all(|line| line.width() <= 6), "{wide:?}");
    assert_eq!(wrap(&[(String::from("a\nb"), Style::new())], 10).len(), 2);
}

#[test]
fn unterminated_code_fences_still_render_while_streaming() {
    let lines = markdown("Partial:\n\n```rust\nfn main() {", 40);
    let text: Vec<String> = lines.iter().map(ToString::to_string).collect();
    assert!(
        text.iter().any(|line| line.contains("fn main() {")),
        "{text:?}"
    );
}

#[test]
fn only_the_newest_lines_the_window_needs_are_rendered() {
    let events: Vec<_> = (1..=500_u64)
        .map(|seq| json!({"seq": seq, "type": "message_accepted", "data": {"sender": "human", "body": format!("message {seq}")}}))
        .collect();
    let model = Model::from_snapshot(&json!({
        "agent": {"id": "a", "state": "idle", "profile": {"provider": "codex"}},
        "events": {"events": events, "cursor": 500, "pruned_through": 0, "history_expired": false},
        "queued": 0, "mail": 0, "children": 0,
    }));
    let (lines, more) = transcript(&model, 40, 20);
    assert!(more && (20..30).contains(&lines.len()), "{}", lines.len());
    assert_eq!(lines.last().unwrap().to_string(), "human: message 500");
    // Asking for everything renders all entries and the history notice on top.
    let (all, more) = transcript(&model, 40, 10_000);
    assert!(!more && all.len() == 501);
    assert_eq!(all[0].to_string(), "(PageUp for older history)");
}

#[test]
fn wrapped_list_items_keep_a_hanging_indent() {
    // Trailing spaces left at a wrap are invisible; compare without them.
    let text = |lines: Vec<Line<'static>>| -> Vec<String> {
        lines
            .iter()
            .map(|line| line.to_string().trim_end().to_owned())
            .collect()
    };
    let bullets = text(markdown(
        "- one two three four five six\n- short\n\n1. alpha beta gamma delta\n   - inner words wrap here too",
        16,
    ));
    assert_eq!(
        bullets,
        [
            "• one two three",
            "  four five six",
            "• short",
            "1. alpha beta",
            "   gamma delta",
            "  • inner words",
            "    wrap here",
            "    too",
        ]
    );
    // A second paragraph and a code block in an item line up under its text.
    let loose = text(markdown(
        "- first part\n\n  second part of it\n\n  ```\n  code\n  ```",
        14,
    ));
    assert_eq!(
        loose,
        ["• first part", "  second part", "  of it", "    code"]
    );
    // In a narrow view the hang is at most half the width.
    let narrow = text(markdown("1. aaaa bbbb cccc", 6));
    assert!(
        narrow.iter().all(|line| line.chars().count() <= 6),
        "{narrow:?}"
    );
    assert!(narrow[1].starts_with("   "), "{narrow:?}");
    // Inside a quote, the hang counts after the bar.
    let quoted = text(markdown("> - one two three", 12));
    assert_eq!(quoted, ["│ • one two", "    three"]);
}

#[test]
fn a_named_agents_header_leads_with_its_name_and_keeps_the_id() {
    let model = Model::from_snapshot(&json!({
        "agent": {"id": "agent-1", "name": "lead", "role": "implementor", "state": "idle",
                  "task_outcome": "open", "model": "gpt", "effort": "low", "auth": "main",
                  "profile": {"provider": "codex"}, "ended_at": null},
        "events": {"events": [], "cursor": 0, "pruned_through": 0, "history_expired": false},
        "queued": 0, "mail": 0, "children": 0,
    }));
    let screen = Screen {
        model: &model,
        scroll: 0,
        input: None,
        focus: Focus::Input,
        card: None,
        status: "",
        completion: None,
        connected: true,
        active: true,
    };
    let rows = render(&screen, 60, 10);
    assert!(
        rows[0].starts_with("lead agent-1 implementor  idle"),
        "{rows:?}"
    );
}
