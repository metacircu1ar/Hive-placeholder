//! The spawn dialog: role, login, model, effort, working directory and task,
//! with the daemon's validation shown at the field it names.
mod render;

use crate::cli::attach::{Kind, Lists};
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::sync::Arc;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Columns of a field's label; values start after it.
pub(super) const LABEL: usize = 10;
/// Columns a focused field keeps for typing before its hint is dropped.
const MIN_TYPING: usize = 8;
const SANDBOXES: [&str; 3] = ["read-only", "workspace-write", "full-access"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Role,
    Name,
    Login,
    Model,
    Effort,
    Sandbox,
    Directory,
    Body,
}

const FIELDS: [Field; 8] = [
    Field::Role,
    Field::Name,
    Field::Login,
    Field::Model,
    Field::Effort,
    Field::Sandbox,
    Field::Directory,
    Field::Body,
];

impl Field {
    fn label(self) -> &'static str {
        match self {
            Self::Role => "Role",
            Self::Name => "Name",
            Self::Login => "Login",
            Self::Model => "Model",
            Self::Effort => "Effort",
            Self::Sandbox => "Sandbox",
            Self::Directory => "Directory",
            Self::Body => "Task",
        }
    }

    /// The field an error names: an input error's `field`, or its code.
    pub(super) fn of_error(error: &Value) -> Option<Self> {
        use crate::error::ErrorCode;
        let error: crate::cli::views::Failure<'_> = crate::cli::views::read(error);
        let by_field = match error.field {
            Some("/role" | "/preset") => Some(Self::Role),
            Some("/name") => Some(Self::Name),
            Some("/auth") => Some(Self::Login),
            Some("/model") => Some(Self::Model),
            Some("/effort") => Some(Self::Effort),
            Some("/sandbox") => Some(Self::Sandbox),
            Some("/cwd") => Some(Self::Directory),
            Some("/body") => Some(Self::Body),
            _ => None,
        };
        by_field.or(match error.code {
            Some(ErrorCode::UnknownRole) => Some(Self::Role),
            Some(ErrorCode::UnknownProfile) => Some(Self::Login),
            Some(ErrorCode::UnknownModel | ErrorCode::ModelMismatch) => Some(Self::Model),
            Some(ErrorCode::UnsupportedEffort | ErrorCode::EffortMismatch) => Some(Self::Effort),
            Some(ErrorCode::InvalidCwd) => Some(Self::Directory),
            _ => None,
        })
    }
}

/// A configured name and the provider it spawns with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Choice {
    pub name: String,
    pub provider: Option<String>,
}

fn choices(reply: &Value, key: &str) -> Vec<Choice> {
    reply[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some(Choice {
                name: row["name"].as_str()?.to_owned(),
                provider: row["provider"].as_str().map(str::to_owned),
            })
        })
        .collect()
}

/// What a dialog key asks for.
#[derive(Debug, PartialEq)]
pub(super) enum Key {
    None,
    /// Send this spawn once, with this fresh request ID.
    Submit {
        id: String,
        request: Value,
    },
    Cancel,
}

#[derive(Debug)]
pub(super) struct Dialog {
    roles: Vec<Choice>,
    logins: Vec<Choice>,
    default_provider: Option<String>,
    /// Index into `roles`, or none.
    role: Option<usize>,
    login: Option<usize>,
    name: String,
    model: String,
    effort: String,
    sandbox: Option<usize>,
    directory: String,
    body: String,
    pub focus: Field,
    error: Option<(Field, String)>,
    pub footer: String,
    /// The roles reply arrived.
    pub loaded: bool,
    /// The request ID of the spawn in flight; keys wait for its reply.
    pub request: Option<String>,
    /// The TUI follows one subtree; a dialog spawn starts outside it.
    scoped: bool,
    /// The models and efforts ← and → cycle through, per provider.
    pub lists: Arc<Lists>,
}

impl Dialog {
    pub(super) fn new(directory: String, scoped: bool) -> Self {
        Self {
            roles: Vec::new(),
            logins: Vec::new(),
            default_provider: None,
            role: None,
            login: None,
            name: String::new(),
            model: String::new(),
            effort: String::new(),
            sandbox: None,
            directory,
            body: String::new(),
            focus: Field::Body,
            error: None,
            footer: String::from("loading roles…"),
            loaded: false,
            request: None,
            scoped,
            lists: Arc::default(),
        }
    }

    /// Apply the `roles` reply: roles, then logins (profiles and groups).
    pub(super) fn roles(&mut self, reply: &Value) {
        self.roles = choices(reply, "roles");
        self.logins = choices(reply, "profiles");
        self.logins.extend(choices(reply, "groups"));
        self.default_provider = reply["default_provider"].as_str().map(str::to_owned);
        self.loaded = true;
        self.footer = Self::hint();
    }

    pub(super) fn roles_failed(&mut self, problem: &str) {
        self.loaded = true;
        self.footer = format!("roles unavailable ({problem}); type a role-free spawn or Esc");
    }

    fn hint() -> String {
        String::from("Tab next field · ←/→ choose · Ctrl-J new line · Enter spawn · Esc cancel")
    }

    /// The provider the selection spawns with, when the config says.
    fn provider(&self) -> Option<&str> {
        let login = self.login.and_then(|index| self.logins.get(index));
        let role = self.role.and_then(|index| self.roles.get(index));
        login
            .and_then(|choice| choice.provider.as_deref())
            .or_else(|| role.and_then(|choice| choice.provider.as_deref()))
            .or(self.default_provider.as_deref())
    }

    pub(super) fn key(&mut self, key: &KeyEvent) -> Key {
        if self.request.is_some() {
            return Key::None;
        }
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return Key::Cancel,
            KeyCode::Tab | KeyCode::Down => self.move_focus(1),
            KeyCode::BackTab | KeyCode::Up => self.move_focus(FIELDS.len() - 1),
            KeyCode::Enter => return self.submit(),
            KeyCode::Left => self.cycle(false),
            KeyCode::Right => self.cycle(true),
            KeyCode::Char('j') if control => {
                if self.focus == Field::Body {
                    self.body.push('\n');
                }
            }
            KeyCode::Backspace => {
                if let Some(text) = self.text() {
                    text.pop();
                }
            }
            KeyCode::Char(character) if !control => {
                if let Some(text) = self.text() {
                    text.push(character);
                }
            }
            _ => {}
        }
        Key::None
    }

    pub(super) fn paste(&mut self, text: &str) {
        let body = self.focus == Field::Body;
        if let Some(field) = self.text() {
            // Only the task keeps line breaks.
            field.extend(
                text.chars()
                    .filter(|character| body && *character == '\n' || !character.is_control()),
            );
        }
    }

    fn move_focus(&mut self, step: usize) {
        let index = FIELDS
            .iter()
            .position(|field| *field == self.focus)
            .unwrap_or(0);
        self.focus = FIELDS[(index + step) % FIELDS.len()];
    }

    /// Cycle a choice: none, then each configured name. Model and effort
    /// cycle through the names listed for the provider the selection resolves
    /// to; a typed value that is not listed counts as none.
    fn cycle(&mut self, forward: bool) {
        let (selected, count) = match self.focus {
            Field::Role => (&mut self.role, self.roles.len()),
            Field::Login => (&mut self.login, self.logins.len()),
            Field::Sandbox => (&mut self.sandbox, SANDBOXES.len()),
            Field::Model => return self.cycle_listed(Kind::Model, forward),
            Field::Effort => return self.cycle_listed(Kind::Effort, forward),
            Field::Name | Field::Directory | Field::Body => return,
        };
        let positions = count + 1;
        let current = selected.map_or(0, |index| index + 1);
        let next = if forward {
            (current + 1) % positions
        } else {
            (current + positions - 1) % positions
        };
        *selected = next.checked_sub(1);
    }

    fn cycle_listed(&mut self, kind: Kind, forward: bool) {
        let names = self.listed(kind).to_vec();
        if names.is_empty() {
            return;
        }
        let text = match kind {
            Kind::Model => &mut self.model,
            Kind::Effort => &mut self.effort,
        };
        let positions = names.len() + 1;
        let current = names
            .iter()
            .position(|name| name == text.trim())
            .map_or(0, |index| index + 1);
        let next = if forward {
            (current + 1) % positions
        } else {
            (current + positions - 1) % positions
        };
        *text = next
            .checked_sub(1)
            .map_or_else(String::new, |index| names[index].clone());
    }

    /// The names listed for the provider the selection resolves to.
    fn listed(&self, kind: Kind) -> &[String] {
        self.provider()
            .map_or(&[], |provider| self.lists.names(provider, kind))
    }

    fn text(&mut self) -> Option<&mut String> {
        match self.focus {
            Field::Name => Some(&mut self.name),
            Field::Model => Some(&mut self.model),
            Field::Effort => Some(&mut self.effort),
            Field::Directory => Some(&mut self.directory),
            Field::Body => Some(&mut self.body),
            Field::Role | Field::Login | Field::Sandbox => None,
        }
    }

    /// The spawn request, or the field that needs correcting first.
    pub(super) fn request(&self) -> Result<Value, (Field, String)> {
        let body = self.body.trim();
        if body.is_empty() {
            return Err((Field::Body, String::from("write the agent's first task")));
        }
        let directory = self.directory.trim();
        if !std::path::Path::new(directory).is_absolute() {
            return Err((Field::Directory, String::from("use an absolute directory")));
        }
        Ok(crate::runtime::api::value(SpawnRequest {
            op: "spawn",
            body,
            cwd: directory,
            role: self
                .role
                .and_then(|i| self.roles.get(i))
                .map(|r| r.name.as_str()),
            auth: self
                .login
                .and_then(|i| self.logins.get(i))
                .map(|r| r.name.as_str()),
            sandbox: self.sandbox.and_then(|i| SANDBOXES.get(i)).copied(),
            name: nonempty(&self.name),
            model: nonempty(&self.model),
            effort: nonempty(&self.effort),
        }))
    }

    fn submit(&mut self) -> Key {
        match self.request() {
            Ok(request) => {
                let id = uuid::Uuid::new_v4().to_string();
                self.error = None;
                self.request = Some(id.clone());
                self.footer = String::from("spawning…");
                Key::Submit { id, request }
            }
            Err((field, problem)) => {
                self.focus = field;
                self.error = Some((field, problem));
                Key::None
            }
        }
    }

    /// The daemon refused the spawn: show it at its field, or in the footer.
    pub(super) fn failed(&mut self, error: &Value) {
        self.request = None;
        let projected: crate::cli::views::Failure<'_> = crate::cli::views::read(error);
        let code = projected
            .code
            .unwrap_or(crate::error::ErrorCode::Internal)
            .as_str();
        let message = projected.message.unwrap_or_default();
        let problem = format!("{code}: {message}");
        if let Some(field) = Field::of_error(error) {
            self.focus = field;
            self.error = Some((field, problem));
            self.footer = Self::hint();
        } else {
            self.error = None;
            self.footer = problem;
        }
    }

    /// A click on a field's row, `offset` columns into the dialog: focus the
    /// field; on a choice, `‹` picks the previous value and `›` the next.
    pub(super) fn click(&mut self, field: Field, offset: u16) {
        if self.request.is_some() {
            return;
        }
        self.focus = field;
        let value = match field {
            Field::Role => self.value(Field::Role),
            Field::Login => self.value(Field::Login),
            Field::Sandbox => self.value(Field::Sandbox),
            _ => return,
        };
        let offset = usize::from(offset);
        if offset == LABEL {
            self.cycle(false);
        } else if offset + 1 == LABEL + value.width() {
            self.cycle(true);
        }
    }
}

/// The dialog as drawn.
#[derive(Debug, Default)]
pub(super) struct Rendered {
    pub lines: Vec<Line<'static>>,
    /// The field each line belongs to.
    pub fields: Vec<Option<Field>>,
    /// Line and column of the cursor in the focused text field.
    pub cursor: Option<(usize, usize)>,
}

impl Rendered {
    fn push(&mut self, line: Line<'static>, field: Option<Field>) {
        self.lines.push(line);
        self.fields.push(field);
    }
}

/// The end of `text` that fits `room` columns with the cursor after it, and
/// the cursor's column within the shown part.
fn scrolled(text: &str, room: usize) -> (String, usize) {
    let width = text.width();
    let shift = width.saturating_sub(room.saturating_sub(1));
    let mut skipped = 0;
    let shown: String = text
        .chars()
        .skip_while(|character| {
            if skipped >= shift {
                return false;
            }
            skipped += character.width().unwrap_or(0);
            true
        })
        .collect();
    let column = shown.width();
    (shown, column)
}

fn choice_text(choice: Option<&Choice>, none: &str) -> String {
    choice.map_or_else(
        || format!("‹ {none} ›"),
        |choice| format!("‹ {} ›", choice.name),
    )
}

fn text_or(text: &str, empty: &str) -> String {
    if text.is_empty() {
        empty.to_owned()
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests;

#[derive(serde::Serialize)]
struct SpawnRequest<'a> {
    op: &'static str,
    body: &'a str,
    cwd: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auth: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sandbox: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<&'a str>,
}
fn nonempty(text: &str) -> Option<&str> {
    let text = text.trim();
    (!text.is_empty()).then_some(text)
}
