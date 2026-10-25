//! The view's state machine: updates and keys in, commands and frames out.
//!
//! `App` never talks to the daemon or the terminal itself, so key handling and
//! reconciliation are tested with recorded updates and synthetic key events.
mod commands;
mod input;
mod render;
mod requests;

use super::{
    card::Card,
    client::{Command, Update},
    model::Model,
    picker::{Kind, Lists, Picker},
    view::{Focus, Panel, Screen, Shown},
};
use crate::cli::views;
use ratatui::{
    Frame,
    crossterm::event::{
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
        MouseEventKind,
    },
    layout::{Position, Rect},
    widgets::{Paragraph, Wrap},
};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// Two Esc presses this close together interrupt the running turn.
const DOUBLE_ESC: Duration = Duration::from_millis(500);
/// How long an ended agent's final state stays on screen with `--exit-on-end`,
/// unless `--linger` says otherwise.
pub(in crate::cli) const END_LINGER: Duration = Duration::from_secs(3);
/// The status while the daemon has no room for another viewer.
pub(super) const WAITING: &str = "waiting for a viewer slot";
/// The status while the picker covers a waiting question.
const HIDDEN_QUESTION: &str = "a question is waiting; Esc closes the picker to show it";

/// The view's own commands with their arguments, shown when they are wrong.
/// Any other `/text` is sent to the agent.
const ARGUMENTS: [(&str, &str); 20] = [
    ("name", "/name NAME"),
    ("model", "/model MODEL"),
    ("effort", "/effort EFFORT"),
    ("options", "/options KEY=VALUE ..."),
    ("auth", "/auth LOGIN"),
    ("usage", "/usage"),
    ("id", "/id"),
    ("stop", "/stop"),
    ("resume", "/resume"),
    ("interrupt", "/interrupt"),
    ("complete", "/complete TEXT"),
    ("terminate", "/terminate"),
    ("branch", "/branch TEXT"),
    ("spawn", "/spawn ROLE TEXT"),
    ("attach", "/attach AGENT"),
    ("accept", "/accept"),
    ("decline", "/decline"),
    ("cancel", "/cancel"),
    ("help", "/help"),
    ("quit", "/quit"),
];

/// The view commands whose names start with `prefix`, in `/help` order.
fn matching(prefix: &str) -> Vec<&'static str> {
    ARGUMENTS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| name.starts_with(prefix))
        .collect()
}

/// The longest start every name shares.
fn common_start(names: &[&str]) -> String {
    let Some((first, rest)) = names.split_first() else {
        return String::new();
    };
    let mut common: &str = first;
    for name in rest {
        // Matching characters have the same bytes, so the shorter length is safe.
        let end = common
            .char_indices()
            .zip(name.chars())
            .find(|((_, a), b)| a != b)
            .map_or_else(|| common.len().min(name.len()), |((index, _), _)| index);
        common = &common[..end];
    }
    common.to_owned()
}

/// What a `/` line was.
#[derive(Debug, PartialEq, Eq)]
enum Handled {
    /// Run (or refused for good); the input is cleared.
    Done,
    /// Wrong arguments; the input stays for correction.
    Kept,
    /// Not a view command: send the line to the agent.
    Forward,
}

/// What the view should do after a key or update.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::cli) enum Exit {
    Quit,
    Attach(String),
    /// The daemon refused the first attach; this is its error reply.
    Refused(Value),
}

/// The follower's connection to the daemon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Link {
    Connected,
    Disconnected,
    /// Refused for capacity; the follower keeps retrying.
    Waiting,
}

/// How the view was opened.
#[derive(Clone, Debug)]
pub(in crate::cli) struct Mode {
    pub read_only: bool,
    pub exit_on_end: bool,
    pub linger: Duration,
    /// Names `/model` and `/effort` offer without an argument.
    pub lists: Arc<Lists>,
}

impl Default for Mode {
    fn default() -> Self {
        Self {
            read_only: false,
            exit_on_end: false,
            linger: END_LINGER,
            lists: Arc::default(),
        }
    }
}

pub(super) struct App {
    target: String,
    pub model: Option<Model>,
    input: String,
    /// Cursor position in characters.
    cursor: usize,
    pub focus: Focus,
    pub scroll: usize,
    pub status: String,
    link: Link,
    mode: Mode,
    card: Option<Card>,
    /// An open `/model` or `/effort` picker, shown in the card's place.
    picker: Option<Picker>,
    actions: u64,
    in_flight: Vec<u64>,
    last_esc: Option<Instant>,
    quit_armed: bool,
    /// Tab cycling through the view commands that match `prefix`: the typed
    /// prefix and the index shown. Any other key ends it.
    completing: Option<(String, usize)>,
    ended: Option<Instant>,
    history_pending: bool,
    /// The key help shown once attached, when the host replaces the default.
    hint: Option<String>,
    /// Focus the approval card as soon as one is shown.
    focus_card: bool,
    /// Transcript size from the last frame, for scrolling and history paging.
    pub shown: Shown,
    /// Commands for the client, drained by the loop after every call.
    pub commands: Vec<Command>,
    pub exit: Option<Exit>,
    /// Text for the terminal's clipboard; the loop owning the terminal sends it.
    clipboard: Option<String>,
}

impl App {
    pub(super) fn new(target: String, mode: Mode) -> Self {
        Self {
            target,
            model: None,
            input: String::new(),
            cursor: 0,
            focus: Focus::Input,
            scroll: 0,
            status: String::from("connecting"),
            link: Link::Connected,
            mode,
            card: None,
            picker: None,
            actions: 0,
            in_flight: Vec::new(),
            last_esc: None,
            quit_armed: false,
            completing: None,
            ended: None,
            history_pending: false,
            hint: None,
            focus_card: false,
            shown: Shown::default(),
            commands: Vec::new(),
            exit: None,
            clipboard: None,
        }
    }

    /// Show the agent's full ID and queue it for the terminal's clipboard. The
    /// status line keeps it visible where the terminal ignores clipboard writes.
    fn copy_id(&mut self) {
        let id = self
            .model
            .as_ref()
            .map(|model| model.header.id.clone())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| self.target.clone());
        self.status = format!("id: {id} (copy sent to the terminal clipboard)");
        self.clipboard = Some(id);
    }

    /// Text `/id` asked to copy, once.
    pub(super) fn take_clipboard(&mut self) -> Option<String> {
        self.clipboard.take()
    }

    /// A write is in flight.
    pub(super) fn busy(&self) -> bool {
        !self.in_flight.is_empty()
    }

    /// The follower is waiting for the daemon to admit another viewer.
    pub(super) fn waiting(&self) -> bool {
        self.link == Link::Waiting
    }

    /// Replace the key help shown once attached.
    pub(super) fn set_hint(&mut self, hint: String) {
        if self.model.is_some() && self.status == self.hint() {
            self.status.clone_from(&hint);
        }
        self.hint = Some(hint);
    }

    /// Focus the approval card now, or as soon as one is shown. An open
    /// picker closes so the question shows.
    pub(super) fn focus_card(&mut self) {
        self.drop_picker();
        if self.card.is_some() {
            self.focus = Focus::Card;
        } else {
            self.focus_card = true;
        }
    }

    /// Leave once an ended agent's linger has passed (`--exit-on-end`).
    pub(super) fn tick(&mut self, now: Instant) {
        if let Some(ended) = self.ended
            && now.duration_since(ended) >= self.mode.linger
        {
            self.exit = Some(Exit::Quit);
        }
    }

    pub(super) fn update(&mut self, update: Update) {
        match update {
            Update::Snapshot(reply) => {
                self.model = Some(Model::from_snapshot(&reply));
                self.link = Link::Connected;
                self.status = self.hint();
            }
            Update::Refresh(reply) => {
                self.link = Link::Connected;
                if let Some(model) = &mut self.model {
                    model.apply_reply(&reply);
                }
            }
            Update::Live(items) => {
                if let Some(model) = &mut self.model {
                    model.apply_live(&items);
                }
            }
            Update::Resync => {
                if let Some(model) = &mut self.model {
                    model.resync();
                }
            }
            Update::Disconnected => {
                self.link = Link::Disconnected;
                if self.model.is_none() {
                    self.status = String::from("the daemon is not answering; retrying");
                }
            }
            Update::Refused(reply) => self.exit = Some(Exit::Refused(reply)),
            Update::Purged => {
                self.status = String::from("the agent was purged; Ctrl-C to leave");
                self.mark_ended();
            }
            Update::Approval(detail) => self.approval(&detail),
            Update::Children(rows) => {
                if let Some(model) = &mut self.model {
                    for row in rows {
                        let row: views::Agent<'_> = views::read(&row);
                        model.child_spawned(row.id.unwrap_or_default(), row.name, row.role);
                    }
                }
            }
            Update::History(page) => {
                self.history_pending = false;
                match page {
                    Ok(page) => {
                        if let Some(model) = &mut self.model {
                            model.prepend(&page);
                        }
                    }
                    Err(problem) => self.status = format!("older history: {problem}"),
                }
            }
            Update::Usage { auth, result } => self.usage_read(&auth, result),
            Update::Problem(problem) => self.status = problem,
            Update::Waiting(waiting) => {
                self.link = if waiting {
                    Link::Waiting
                } else {
                    Link::Connected
                };
                if waiting {
                    self.status = String::from(WAITING);
                } else if self.status == WAITING {
                    self.status = self.hint();
                }
            }
            Update::Done {
                action,
                label,
                result,
            } => self.done(action, &label, result),
        }
        if self.model.as_ref().is_some_and(|model| model.header.ended) {
            self.mark_ended();
        }
    }

    fn mark_ended(&mut self) {
        if self.mode.exit_on_end && self.ended.is_none() {
            self.ended = Some(Instant::now());
        }
    }

    fn approval(&mut self, detail: &Value) {
        self.card = Card::from_detail(detail);
        if !self.has_card() && self.focus == Focus::Card {
            self.focus = Focus::Input;
        }
        if self.card.is_some() && std::mem::take(&mut self.focus_card) {
            // The jump to this question was asked for: it shows, not a picker.
            self.drop_picker();
            self.focus = Focus::Card;
        }
        self.note_hidden_question();
        if self.card.is_some() && self.mode.read_only {
            self.status = String::from("a question is waiting; answer it from another view");
        }
    }

    fn done(&mut self, action: u64, label: &str, result: Result<Value, String>) {
        self.in_flight.retain(|pending| *pending != action);
        self.status = match result {
            Ok(reply) => match views::read::<views::AgentIdentity<'_>>(&reply).agent_id {
                Some(id) if label == "spawn" || label == "branch" => {
                    format!("{label}: started {id}; /attach {id} opens it")
                }
                _ => format!("{label}: ok"),
            },
            Err(error) => format!("{label} failed: {error}"),
        };
    }

    fn hint(&self) -> String {
        if let Some(hint) = &self.hint {
            hint.clone()
        } else if self.mode.read_only {
            String::from("read-only · PgUp/PgDn scroll · Ctrl-C quit")
        } else {
            String::from(
                "Enter send/steer · Alt-Enter queue · Ctrl-R redirect · Esc Esc interrupt · \
                    Tab focus · Ctrl-C quit",
            )
        }
    }
}

/// `k=v` pairs; a value that parses as JSON keeps its type.
fn options(text: &str) -> Option<serde_json::Map<String, Value>> {
    text.split_whitespace()
        .map(|pair| {
            let (key, value) = pair.split_once('=')?;
            let value = serde_json::from_str(value).unwrap_or_else(|_| Value::from(value));
            (!key.is_empty()).then(|| (key.to_owned(), value))
        })
        .collect()
}

#[cfg(test)]
mod tests;
