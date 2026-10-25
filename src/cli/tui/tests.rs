use super::*;
use ratatui::{Terminal, backend::TestBackend, crossterm::event::MouseEventKind};

const A: &str = "aaaaaaaa-0000-4000-8000-000000000001";
const B: &str = "bbbbbbbb-0000-4000-8000-000000000002";

fn args(limit: u16) -> Tui {
    Tui {
        root: None,
        limit,
        keep_ended: false,
        quiet: false,
        no_mouse: false,
    }
}

fn console(args: &Tui) -> Console<'_> {
    let (asks, _) = std::sync::mpsc::channel();
    let (_, answers) = std::sync::mpsc::channel();
    Console::new(
        PathBuf::from("/nonexistent/hive-tui-test"),
        Prefix::default(),
        args,
        asks,
        answers,
    )
}

fn tree(approval: Option<&str>) -> Feed {
    Feed::Tree(json!({"agents": [
        {"id": A, "parent_id": null, "role": "lead", "state": "running", "task_outcome": "open",
         "waiting_reason": null, "approval_id": approval, "ended_at": null},
        {"id": B, "parent_id": A, "role": "helper", "state": "idle", "task_outcome": "open",
         "waiting_reason": null, "approval_id": null, "ended_at": null},
    ], "truncated": false, "version": {"epoch": 1, "revision": 1}}))
}

fn snapshot(id: &str, role: &str) -> Value {
    json!({
        "agent": {"id": id, "role": role, "state": "idle", "task_outcome": "open", "model": "m",
                  "effort": "low", "auth": "main", "profile": {"provider": "codex"}, "ended_at": null},
        "events": {"events": [{"seq": 1, "type": "message_accepted",
                               "data": {"sender": "human", "body": format!("hello {role}")}}],
                   "cursor": 1, "pruned_through": 0, "history_expired": false},
        "queued": 0, "mail": 0, "children": 0,
    })
}

/// Show both agents in a left/right split without a daemon.
fn split(console: &mut Console<'_>) {
    for (id, role, place) in [
        (A, "lead", Place::Tab),
        (B, "helper", Place::Split(layout::Split::Columns)),
    ] {
        let view = View::detached(id, console.mode(), &snapshot(id, role));
        console.views.insert(id.to_owned(), view);
        console.layout.open(id, place, Room::Free, |_| false);
    }
    console.focus = Focus::View;
}

fn key(console: &mut Console<'_>, code: KeyCode, modifiers: KeyModifiers) {
    console.event(&Event::Key(KeyEvent::new(code, modifiers)), Instant::now());
}

fn prefixed(console: &mut Console<'_>, character: char) {
    key(console, KeyCode::Char('a'), KeyModifiers::CONTROL);
    key(console, KeyCode::Char(character), KeyModifiers::NONE);
}

fn render(console: &mut Console<'_>) -> (Vec<String>, Terminal<TestBackend>) {
    let (width, height) = (110, 24);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| console.draw(frame)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let rows = (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
        })
        .collect();
    (rows, terminal)
}

#[test]
fn a_split_layout_shows_the_sidebar_both_views_and_one_cursor() {
    let args = args(6);
    let mut console = console(&args);
    console.feed(tree(None));
    split(&mut console);
    let (rows, terminal) = render(&mut console);
    let text = rows.join("\n");
    for expected in [
        " agents 2 ",
        "● lead aaaaaaaa",
        "○ helper bbbbbbbb",
        " 1:lead|helper ",
        " lead aaaaaaaa ",
        " helper bbbbbbbb ",
        "human: hello lead",
        "human: hello helper",
        "Ctrl-A then: s sidebar",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in\n{text}");
    }
    // The helper sits under its parent in the tree.
    let helper = rows
        .iter()
        .position(|row| row.contains("  ○ helper"))
        .unwrap();
    let lead = rows.iter().position(|row| row.contains("▾● lead")).unwrap();
    assert_eq!(helper, lead + 1, "{text}");
    // Only the active (right) view places the cursor.
    let backend = terminal.backend();
    assert!(backend.cursor_visible());
    let width = (110_u16 / 4).clamp(20, 36);
    assert!(
        backend.cursor_position().x > width + (110 - width) / 2,
        "{:?}",
        backend.cursor_position()
    );
    // With the sidebar focused, no view owns the cursor.
    prefixed(&mut console, 's');
    assert_eq!(console.focus, Focus::Sidebar);
    let (_, terminal) = render(&mut console);
    assert!(!terminal.backend().cursor_visible());
}

#[test]
fn new_approvals_notify_once_and_fill_the_attention_queue() {
    let args = args(6);
    let mut console = console(&args);
    console.feed(tree(Some("q1")));
    assert_eq!(console.notify.take(), Some(1));
    console.feed(tree(Some("q1")));
    assert_eq!(console.notify.take(), None);
    console.feed(tree(Some("q2")));
    assert_eq!(console.notify.take(), Some(1));
    assert_eq!(console.attention.entries.len(), 1);
    let (rows, _) = render(&mut console);
    assert!(rows.join("\n").contains("! 1 need you"));
    console.feed(tree(None));
    assert!(console.attention.entries.is_empty());
}

#[test]
fn prefix_keys_move_focus_switch_views_and_close_them() {
    let args = args(6);
    let mut console = console(&args);
    console.feed(tree(None));
    split(&mut console);
    assert_eq!(console.layout.active(), Some(B));
    prefixed(&mut console, 'o');
    assert_eq!(console.layout.active(), Some(A));
    prefixed(&mut console, 's');
    assert_eq!(console.focus, Focus::Sidebar);
    key(&mut console, KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(console.focus, Focus::View);
    // An unbound key after the prefix reaches nobody.
    prefixed(&mut console, 'z');
    assert!(!console.views[A].busy());
    // A write in flight makes closing ask once.
    console.event(&Event::Paste(String::from("reply: hi")), Instant::now());
    key(&mut console, KeyCode::Enter, KeyModifiers::NONE);
    assert!(console.views[A].busy());
    prefixed(&mut console, 'w');
    assert!(console.views.contains_key(A));
    assert!(
        console.status.contains("again closes"),
        "{}",
        console.status
    );
    prefixed(&mut console, 'w');
    assert!(!console.views.contains_key(A));
    assert_eq!(console.layout.active(), Some(B));
    // An idle view closes at once; with none left the sidebar has focus.
    prefixed(&mut console, 'w');
    assert!(console.views.is_empty() && console.focus == Focus::Sidebar);
    assert_eq!(console.closed.len(), 2);
}

#[test]
fn control_c_asks_once_while_a_write_is_in_flight() {
    let args = args(6);
    let mut console = console(&args);
    split(&mut console);
    console.event(&Event::Paste(String::from("reply: hi")), Instant::now());
    key(&mut console, KeyCode::Enter, KeyModifiers::NONE);
    key(&mut console, KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(!console.quit);
    // Any other key disarms the confirmation.
    key(&mut console, KeyCode::Left, KeyModifiers::NONE);
    key(&mut console, KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(!console.quit);
    key(&mut console, KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(console.quit);
}

#[test]
fn the_view_cap_replaces_once_then_waits_for_closed_polls() {
    let args = args(1);
    let mut console = console(&args);
    let now = Instant::now();
    assert!(console.open(A, None, now));
    // At the cap, the idle view is replaced; its poll still counts.
    assert!(console.open(B, None, now));
    assert!(!console.views.contains_key(A) && console.views.contains_key(B));
    assert_eq!(console.closed.len(), 1);
    assert!(!console.open(A, None, now));
    assert!(
        console.status.contains("retry shortly"),
        "{}",
        console.status
    );
    // Once the closed poll has surely ended, there is room again.
    assert!(console.open(A, None, now + CLOSED_POLL));
    assert_eq!(console.closed.len(), 1);
}

#[test]
fn the_prefix_twice_reaches_the_view_and_dropped_keys_do_not() {
    let args = args(6);
    let esc = |console: &mut Console<'_>| key(console, KeyCode::Esc, KeyModifiers::NONE);
    let prefix =
        |console: &mut Console<'_>| key(console, KeyCode::Char('a'), KeyModifiers::CONTROL);
    // Esc Esc interrupts: the view sends a write.
    let mut control = console(&args);
    split(&mut control);
    esc(&mut control);
    esc(&mut control);
    assert!(control.views[B].busy());
    // The passed-through prefix key reaches the view between the two Escs.
    let mut passed = console(&args);
    split(&mut passed);
    esc(&mut passed);
    prefix(&mut passed);
    prefix(&mut passed);
    esc(&mut passed);
    assert!(!passed.views[B].busy());
    // An unbound key after the prefix never reaches it.
    let mut dropped = console(&args);
    split(&mut dropped);
    esc(&mut dropped);
    prefix(&mut dropped);
    key(&mut dropped, KeyCode::Char('z'), KeyModifiers::NONE);
    esc(&mut dropped);
    assert!(dropped.views[B].busy());
}

#[test]
fn refused_views_free_their_slot_and_quit_views_hold_it() {
    let args = args(6);
    let mut console = console(&args);
    split(&mut console);
    console
        .views
        .get_mut(A)
        .unwrap()
        .refuse(json!({"error": {"code": "unknown_agent", "message": "No agent."}}));
    console.pump(Instant::now());
    assert!(!console.views.contains_key(A) && console.closed.is_empty());
    assert_eq!(console.status, "cannot open aaaaaaaa: unknown_agent");
    console.event(&Event::Paste(String::from("/quit")), Instant::now());
    key(&mut console, KeyCode::Enter, KeyModifiers::NONE);
    console.pump(Instant::now());
    assert!(console.views.is_empty());
    assert_eq!(console.closed.len(), 1);
}

#[test]
fn spawn_replies_report_even_after_the_dialog_was_left() {
    let args = args(6);
    let mut first = console(&args);
    let now = Instant::now();
    first.answer(
        Answer::Spawned {
            id: String::from("old"),
            result: Err(json!({"code": "agent_limit", "message": "Full."})),
        },
        now,
    );
    assert_eq!(first.status, "spawn failed: agent_limit: Full.");
    // A newer dialog is not touched by an older submission's reply.
    first.overlay = Overlay::Spawn(Box::new(spawn::Dialog::new(String::from("/w"), false)));
    first.answer(
        Answer::Spawned {
            id: String::from("old"),
            result: Ok(json!({"agent_id": A})),
        },
        now,
    );
    assert!(matches!(&first.overlay, Overlay::Spawn(dialog) if dialog.request.is_none()));
    assert!(first.views.contains_key(A));
    assert_eq!(first.status, "started aaaaaaaa");
    // The dialog's own reply closes it; a view refused at the cap keeps the ID.
    let small = args_with_root(1);
    let mut scoped = console(&small);
    scoped.open(B, None, now);
    scoped.open(A, None, now);
    let mut dialog = spawn::Dialog::new(String::from("/w"), true);
    dialog.request = Some(String::from("mine"));
    scoped.overlay = Overlay::Spawn(Box::new(dialog));
    scoped.answer(
        Answer::Spawned {
            id: String::from("mine"),
            result: Ok(json!({"agent_id": "cccccccc-0000-4000-8000-000000000003"})),
        },
        now,
    );
    assert!(matches!(scoped.overlay, Overlay::None));
    assert!(
        scoped
            .status
            .starts_with("started cccccccc-0000-4000-8000-000000000003: outside this TUI's scope")
            && scoped.status.contains("retry shortly"),
        "{}",
        scoped.status
    );
}

fn args_with_root(limit: u16) -> Tui {
    Tui {
        root: Some(String::from(A)),
        ..args(limit)
    }
}

fn mouse(console: &mut Console<'_>, kind: MouseEventKind, (column, row): (u16, u16)) -> bool {
    console.event(
        &Event::Mouse(ratatui::crossterm::event::MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }),
        Instant::now(),
    )
}

fn click(console: &mut Console<'_>, at: (u16, u16)) -> bool {
    mouse(
        console,
        MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left),
        at,
    )
}

/// The cell where `text` first appears on the rendered screen.
fn find(rows: &[String], text: &str) -> (u16, u16) {
    rows.iter()
        .enumerate()
        .find_map(|(y, row)| {
            let byte = row.find(text)?;
            let x = row[..byte].chars().count();
            Some((u16::try_from(x).unwrap(), u16::try_from(y).unwrap()))
        })
        .unwrap_or_else(|| panic!("{text:?} not on screen:\n{}", rows.join("\n")))
}

#[test]
fn a_sidebar_click_opens_the_agent_and_its_marker_collapses_instead() {
    let args = args(6);
    let mut console = console(&args);
    console.feed(tree(None));
    console.sidebar.filter = String::from("helper");
    let (rows, _) = render(&mut console);
    // The filtered row opens its agent and clears the filter, as Enter does.
    assert!(click(&mut console, find(&rows, "helper bbbbbbbb")));
    assert!(console.views.contains_key(B));
    assert_eq!(
        (console.focus, console.layout.active()),
        (Focus::View, Some(B))
    );
    assert!(console.sidebar.filter.is_empty());
    // On the tree, the parent's marker collapses its subtree.
    let (rows, _) = render(&mut console);
    let (x, y) = find(&rows, "▾● lead");
    assert!(click(&mut console, (x, y)));
    assert_eq!(console.focus, Focus::Sidebar);
    assert_eq!(console.views.len(), 1);
    let (rows, _) = render(&mut console);
    assert!(
        rows.join("\n").contains("▸● lead aaaaaaaa +1"),
        "{}",
        rows.join("\n")
    );
    // Below the rows, a click only focuses the sidebar.
    console.focus = Focus::View;
    assert!(click(&mut console, (2, 20)));
    assert_eq!(console.focus, Focus::Sidebar);
}

#[test]
fn clicks_switch_tabs_activate_views_and_the_wheel_scrolls_without_focus() {
    let args = args(6);
    let mut console = console(&args);
    console.feed(tree(None));
    // A long transcript in the first view, so scrolling shows older lines.
    let mut long = snapshot(A, "lead");
    let events: Vec<Value> = (1..=60)
        .map(|n| {
            json!({"seq": n, "type": "message_accepted",
            "data": {"sender": "human", "body": format!("line {n:02}")}})
        })
        .collect();
    long["events"]["events"] = json!(events);
    long["events"]["cursor"] = json!(60);
    console
        .views
        .insert(A.to_owned(), View::detached(A, console.mode(), &long));
    console.layout.open(A, Place::Tab, Room::Free, |_| false);
    let helper = View::detached(B, console.mode(), &snapshot(B, "helper"));
    console.views.insert(B.to_owned(), helper);
    console
        .layout
        .open(B, Place::Split(layout::Split::Columns), Room::Free, |_| {
            false
        });
    console.focus = Focus::View;
    let (rows, _) = render(&mut console);
    assert!(!rows.join("\n").contains("line 30"));
    // The wheel over the inactive left view scrolls it; the right stays active.
    let left = find(&rows, "human: line 60");
    for _ in 0..5 {
        assert!(mouse(&mut console, MouseEventKind::ScrollUp, left));
    }
    assert_eq!(console.layout.active(), Some(B));
    let (rows, _) = render(&mut console);
    assert!(rows.join("\n").contains("line 4"), "{}", rows.join("\n"));
    // A click in the left view activates it.
    assert!(click(&mut console, left));
    assert_eq!(console.layout.active(), Some(A));
    // A second tab, then a click on the first tab's label goes back.
    let third = "cccccccc-0000-4000-8000-000000000003";
    console.views.insert(
        third.to_owned(),
        View::detached(third, console.mode(), &snapshot(third, "third")),
    );
    console
        .layout
        .open(third, Place::Tab, Room::Free, |_| false);
    assert_eq!(console.layout.current, 1);
    let (rows, _) = render(&mut console);
    assert!(click(&mut console, find(&rows, " 1:lead|helper ")));
    assert_eq!(console.layout.current, 0);
    // Releases, drags and moves change nothing.
    for kind in [
        MouseEventKind::Up(ratatui::crossterm::event::MouseButton::Left),
        MouseEventKind::Moved,
        MouseEventKind::Drag(ratatui::crossterm::event::MouseButton::Left),
    ] {
        assert!(!mouse(&mut console, kind, left));
    }
}

#[test]
fn overlay_clicks_open_questions_close_outside_and_edit_the_dialog() {
    let args = args(6);
    let mut console = console(&args);
    console.feed(tree(Some("q1")));
    console.overlay = Overlay::Attention;
    let (rows, _) = render(&mut console);
    // The sidebar shows the same label; the list is to its right.
    let (x, y) = find(&rows, "needs you (1)");
    assert!(click(&mut console, (x, y + 1)));
    assert!(matches!(console.overlay, Overlay::None));
    assert_eq!(console.layout.active(), Some(A));
    // A click outside the list closes it.
    console.overlay = Overlay::Attention;
    render(&mut console);
    assert!(click(&mut console, (0, 0)));
    assert!(matches!(console.overlay, Overlay::None));
    // In the dialog, ‹ and › cycle a choice; outside clicks keep it open.
    let mut dialog = spawn::Dialog::new(String::from("/w"), false);
    dialog.roles(
        &json!({"roles": [{"name": "implementor", "provider": "codex"},
        {"name": "reviewer", "provider": "codex"}], "profiles": [], "groups": []}),
    );
    console.overlay = Overlay::Spawn(Box::new(dialog));
    let (rows, _) = render(&mut console);
    let (x, y) = find(&rows, "‹ (none) ›");
    assert!(click(&mut console, (x, y)));
    assert!(click(&mut console, (0, 0)));
    let Overlay::Spawn(dialog) = &console.overlay else {
        panic!("the dialog closed");
    };
    let text: Vec<String> = dialog
        .lines(74)
        .lines
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(
        text.iter().any(|line| line.contains("‹ reviewer ›")),
        "{text:?}"
    );
    assert_eq!(dialog.focus, spawn::Field::Role);
    let (rows, _) = render(&mut console);
    let (x, y) = find(&rows, "‹ reviewer ›");
    let next = x + u16::try_from("‹ reviewer ›".chars().count()).unwrap() - 1;
    assert!(click(&mut console, (next, y)));
    let Overlay::Spawn(dialog) = &console.overlay else {
        panic!("the dialog closed");
    };
    let text: Vec<String> = dialog
        .lines(74)
        .lines
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(
        text.iter().any(|line| line.contains("‹ (none) ›")),
        "{text:?}"
    );
}

#[test]
fn clicks_disarm_confirmations_and_end_a_pending_prefix() {
    let args = args(6);
    let mut console = console(&args);
    split(&mut console);
    console.armed = Some(Armed::Quit);
    key(&mut console, KeyCode::Char('a'), KeyModifiers::CONTROL);
    console.armed = Some(Armed::Quit);
    assert!(console.router.pending());
    render(&mut console);
    assert!(click(&mut console, (0, 0)));
    assert!(console.armed.is_none() && !console.router.pending());
}

#[test]
fn clicking_away_from_the_sidebar_drops_a_pending_pick() {
    let args = args(6);
    let mut console = console(&args);
    console.feed(tree(None));
    split(&mut console);
    prefixed(&mut console, 't');
    assert_eq!(console.place, Some(Place::Tab));
    let (rows, _) = render(&mut console);
    assert!(click(&mut console, find(&rows, "human: hello lead")));
    assert_eq!(console.place, None);
    assert_eq!(console.status, console.hint());
}

#[test]
fn the_button_bar_runs_the_prefix_actions() {
    let args = args(6);
    let mut console = console(&args);
    console.feed(tree(None));
    console.views.insert(
        A.to_owned(),
        View::detached(A, console.mode(), &snapshot(A, "lead")),
    );
    console.layout.open(A, Place::Tab, Room::Free, |_| false);
    console.focus = Focus::View;
    let (rows, _) = render(&mut console);
    let text = rows.join("\n");
    assert!(text.contains(" Quit  Ctrl-A: more keys"), "{text}");
    assert!(click(&mut console, find(&rows, " Questions ")));
    assert!(matches!(console.overlay, Overlay::Attention));
    let (rows, _) = render(&mut console);
    assert!(click(&mut console, find(&rows, " Split ")));
    assert!(matches!(console.overlay, Overlay::None));
    assert_eq!(console.focus, Focus::Sidebar);
    assert!(
        console.status.contains("pick an agent"),
        "{}",
        console.status
    );
    // Quit asks once while a write is in flight, then quits on a second click.
    console.focus = Focus::View;
    console.event(&Event::Paste(String::from("reply: hi")), Instant::now());
    key(&mut console, KeyCode::Enter, KeyModifiers::NONE);
    let (rows, _) = render(&mut console);
    let quit = find(&rows, " Quit ");
    assert!(click(&mut console, quit));
    assert!(!console.quit);
    assert!(click(&mut console, quit));
    assert!(console.quit);
}
