//! Lifecycle regressions.
use super::*;

#[test]
fn tab_completes_view_commands_and_the_status_line_guides_typing() {
    let mut app = app("idle", false);
    let tab = |app: &mut App| press(app, KeyCode::Tab, KeyModifiers::NONE);
    // A single match is filled in, with a space when it takes arguments.
    typed(&mut app, "/mo");
    assert_eq!(status(&app), "Tab completes: /model");
    tab(&mut app);
    assert_eq!((app.input.as_str(), app.focus), ("/model ", Focus::Input));
    assert_eq!(app.cursor, app.input.chars().count());
    assert_eq!(status(&app), "usage: /model MODEL");
    // A command without arguments gets none.
    let mut app = self::app("idle", false);
    typed(&mut app, "/dec");
    tab(&mut app);
    assert_eq!(app.input, "/decline");
    assert_eq!(status(&app), "usage: /decline");
    // Several matches extend to their common start, then Tab cycles.
    let mut app = self::app("idle", false);
    typed(&mut app, "/c");
    assert_eq!(status(&app), "Tab completes: /complete /cancel");
    tab(&mut app);
    assert_eq!(app.input, "/complete");
    tab(&mut app);
    assert_eq!(app.input, "/cancel");
    assert_eq!(status(&app), "usage: /cancel");
    tab(&mut app);
    assert_eq!(app.input, "/complete");
    // Any other key ends cycling; the next Tab starts from the new text.
    press(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
    tab(&mut app);
    assert_eq!(app.input, "/complete ");
    let mut app = self::app("idle", false);
    typed(&mut app, "/s");
    tab(&mut app);
    assert_eq!(app.input, "/stop");
    tab(&mut app);
    assert_eq!(app.input, "/spawn");
    // "/i" matches /id and /interrupt: Tab cycles between them.
    let mut app = self::app("idle", false);
    typed(&mut app, "/i");
    assert_eq!(status(&app), "Tab completes: /id /interrupt");
    tab(&mut app);
    assert_eq!(app.input, "/id");
    tab(&mut app);
    assert_eq!(app.input, "/interrupt");
    // An unknown command is left alone for the agent, and focus stays.
    let mut app = self::app("idle", false);
    typed(&mut app, "/review");
    tab(&mut app);
    assert_eq!((app.input.as_str(), app.focus), ("/review", Focus::Input));
    assert!(
        status(&app).contains("Enter sends it to the agent"),
        "{}",
        status(&app)
    );
    // Ordinary text, a literal //, or a cursor inside the word moves focus.
    for line in ["hello", "//mo", "/model gpt"] {
        let mut app = self::app("idle", false);
        typed(&mut app, line);
        tab(&mut app);
        assert_eq!(
            (app.input.as_str(), app.focus),
            (line, Focus::Transcript),
            "{line}"
        );
    }
    let mut app = self::app("idle", false);
    typed(&mut app, "/mo");
    press(&mut app, KeyCode::Left, KeyModifiers::NONE);
    tab(&mut app);
    assert_eq!((app.input.as_str(), app.focus), ("/mo", Focus::Transcript));
    // Without a slash line there is no help; "/" alone lists every command.
    let mut app = self::app("idle", false);
    assert_eq!(status(&app), "");
    typed(&mut app, "/");
    assert!(
        status(&app).starts_with("Tab completes: /name /model"),
        "{}",
        status(&app)
    );
}

#[test]
fn completion_help_never_hides_the_status_line() {
    use ratatui::{Terminal, backend::TestBackend};
    let mut app = app("idle", false);
    typed(&mut app, "/mo");
    // Messages that arrive while a command is typed stay on the status line.
    app.update(Update::Problem(String::from(
        "the daemon refused the write",
    )));
    let screen = app.screen(true, Rect::new(0, 0, 80, 24)).unwrap();
    assert_eq!(screen.status, "the daemon refused the write");
    assert_eq!(screen.completion.as_deref(), Some("Tab completes: /model"));
    // The Ctrl-C confirmation for a write in flight is shown too.
    typed(&mut app, "del");
    app.act("x", json!({"op": "ping"}));
    press(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(app.status.contains("Ctrl-C again"), "{}", app.status);
    assert_eq!(
        app.screen(true, Rect::new(0, 0, 80, 24)).unwrap().status,
        app.status
    );
    // The help is drawn on the input's top border.
    let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
    terminal
        .draw(|frame| app.draw_into(frame, Rect::new(0, 0, 60, 24), true))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let row = app.shown.regions.input.y;
    let border: String = (0..60)
        .map(|x| buffer[(x, row)].symbol().to_owned())
        .collect();
    assert!(border.contains("usage: /model MODEL"), "{border}");
}
