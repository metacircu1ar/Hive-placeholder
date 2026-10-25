use super::super::model::Entry;
use super::*;
use ratatui::crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

fn snapshot(state: &str, steering: bool) -> Update {
    Update::Snapshot(json!({
        "agent": {"id": "a", "state": state, "task_outcome": "open", "model": "m",
                  "effort": "low", "auth": "main", "profile": {"provider": "codex"},
                  "steering_supported": steering, "ended_at": null},
        "events": {"events": [{"seq": 4, "type": "message_accepted",
                               "data": {"sender": "human", "body": "x"}}],
                   "cursor": 4, "pruned_through": 0, "history_expired": false},
        "queued": 0, "mail": 0, "children": 0,
    }))
}

fn app(state: &str, steering: bool) -> App {
    let mut app = App::new(String::from("a"), Mode::default());
    app.update(snapshot(state, steering));
    app
}

fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    app.event(&Event::Key(KeyEvent::new(code, modifiers)));
}

fn typed(app: &mut App, text: &str) {
    app.event(&Event::Paste(text.to_owned()));
}

/// The writes queued so far, as (action, request ID, request).
fn writes(app: &mut App) -> Vec<(u64, String, Value)> {
    app.commands
        .drain(..)
        .filter_map(|command| match command {
            Command::Write {
                action,
                id,
                request,
                ..
            } => Some((action, id, request)),
            Command::History { .. } | Command::Usage { .. } => None,
        })
        .collect()
}

fn mouse_at(app: &mut App, kind: MouseEventKind, (column, row): (u16, u16)) -> bool {
    app.event(&Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }))
}

fn listed_app(models: &[&str]) -> App {
    let owned = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
    let lists = Lists {
        codex: super::super::picker::List {
            models: owned(models),
            efforts: owned(&["low", "high"]),
        },
        ..Lists::default()
    };
    let mut app = App::new(
        String::from("a"),
        Mode {
            lists: Arc::new(lists),
            ..Mode::default()
        },
    );
    app.update(snapshot("idle", false));
    app
}

/// The card's frame title and lines.
fn card_text(app: &App) -> Option<(String, Vec<String>)> {
    let screen = app.screen(true, Rect::new(0, 0, 60, 24))?;
    let panel = screen.card?;
    Some((
        panel.title,
        panel.lines.iter().map(ToString::to_string).collect(),
    ))
}

fn run(app: &mut App, line: &str) {
    typed(app, line);
    press(app, KeyCode::Enter, KeyModifiers::NONE);
}

/// The input border's help for the view command being typed.
fn status(app: &App) -> String {
    app.screen(true, Rect::new(0, 0, 80, 24))
        .unwrap()
        .completion
        .unwrap_or_default()
}

mod cases_authentication_1;
mod cases_behaviour_1;
mod cases_indexing_1;
mod cases_lifecycle_1;
mod cases_presentation_1;
mod cases_requests_1;
mod cases_transport_1;
