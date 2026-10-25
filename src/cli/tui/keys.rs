//! The prefix key: read from `tui.toml`, matched under every key encoding, and
//! the key after it mapped to a TUI action. The file also lists the models and
//! efforts the views offer.
use crate::cli::attach::{List, Lists, MAX_LISTED};
use crate::error::{Error, Result};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::{
    path::Path,
    time::{Duration, Instant},
};

/// A pending prefix expires after this long.
pub(super) const PREFIX_TIMEOUT: Duration = Duration::from_millis(1500);
/// A letter: it is on every keyboard layout, and neither tmux (Ctrl-B) nor
/// zellij binds it by default.
const DEFAULT_PREFIX: &str = "ctrl-a";
const UNREADABLE: &str = "Cannot read tui.toml beside the configuration file.";
const INVALID_FILE: &str = "Invalid tui.toml; its settings are prefix, a string, mouse, a \
    boolean, and [codex] and [claude] tables with models and efforts lists of strings.";
const INVALID_CODEX_MODELS: &str = "Invalid tui.toml [codex] models; list at most 32 distinct \
    names of 1-128 ASCII letters, digits or -_.:@/[], without a leading '-'.";
const INVALID_CLAUDE_MODELS: &str = "Invalid tui.toml [claude] models; list at most 32 \
    distinct names of 1-128 ASCII letters, digits or -_.:@/[], without a leading '-'.";
const INVALID_CODEX_EFFORTS: &str = "Invalid tui.toml [codex] efforts; list distinct values \
    from none, minimal, low, medium, high, xhigh, max and ultra.";
const INVALID_CLAUDE_EFFORTS: &str = "Invalid tui.toml [claude] efforts; list distinct values \
    from none, minimal, low, medium, high, xhigh, max and ultra.";
const INVALID_PREFIX: &str = "Invalid tui.toml prefix; use ctrl- with a letter, ], ^, _ or \
    backslash, except ctrl-c, h, i, j, m and r.";

/// A Ctrl+character prefix. Under the legacy key encoding (the only one tmux
/// and zellij pass on), bytes 0x1C–0x1F arrive as Ctrl-4 to Ctrl-7.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Prefix {
    character: char,
    alias: Option<char>,
}

impl Default for Prefix {
    fn default() -> Self {
        Self {
            character: 'a',
            alias: None,
        }
    }
}

impl Prefix {
    /// `ctrl-]` and the like; keys that T1 uses, or that terminals cannot tell
    /// apart from Tab, Enter, Esc, Backspace or a newline, are refused.
    pub(super) fn parse(text: &str) -> Option<Self> {
        let text = text.trim().to_ascii_lowercase();
        let mut characters = text.strip_prefix("ctrl-")?.chars();
        let character = characters.next()?;
        if characters.next().is_some() {
            return None;
        }
        let character = match character {
            '4' => '\\',
            '5' => ']',
            '6' => '^',
            '7' => '_',
            'c' | 'h' | 'i' | 'j' | 'm' | 'r' => return None,
            other => other,
        };
        let alias = match character {
            '\\' => Some('4'),
            ']' => Some('5'),
            '^' => Some('6'),
            '_' => Some('7'),
            'a'..='z' => None,
            _ => return None,
        };
        Some(Self { character, alias })
    }

    pub(super) fn matches(self, key: &KeyEvent) -> bool {
        if !key.modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }
        let KeyCode::Char(character) = key.code else {
            return false;
        };
        let character = character.to_ascii_lowercase();
        character == self.character || Some(character) == self.alias
    }

    /// How the status line names the prefix, such as `Ctrl-A`.
    pub(super) fn label(self) -> String {
        format!("Ctrl-{}", self.character.to_ascii_uppercase())
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    prefix: Option<String>,
    mouse: Option<bool>,
    codex: Option<ProviderFile>,
    claude: Option<ProviderFile>,
}

/// One provider's table: the names views offer for its agents.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderFile {
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    efforts: Vec<String>,
}

/// What `tui.toml` sets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Settings {
    pub prefix: Prefix,
    /// Clicks and the wheel go to the TUI (the default).
    pub mouse: bool,
    pub lists: Lists,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            prefix: Prefix::default(),
            mouse: true,
            lists: Lists::default(),
        }
    }
}

/// The settings in `tui.toml` in `directory`; a missing file means the defaults.
pub(super) fn load(directory: &Path) -> Result<Settings> {
    let path = directory.join("tui.toml");
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Settings::default());
        }
        Err(cause) => return Err(Error::config_unavailable(UNREADABLE).with_source(cause)),
    };
    let data = crate::os::read_capped(file, crate::config::CONFIG_READ_BYTES)
        .map_err(|cause| Error::config_unavailable(UNREADABLE).with_source(cause))?;
    parse(&data)
}

pub(super) fn parse(data: &[u8]) -> Result<Settings> {
    if data.len() > crate::config::CONFIG_BYTES {
        return Err(Error::config_invalid(INVALID_FILE));
    }
    let text = std::str::from_utf8(data)
        .map_err(|cause| Error::config_invalid(INVALID_FILE).with_source(cause))?;
    let file: File = toml::from_str(text)
        .map_err(|cause| Error::config_invalid(INVALID_FILE).with_source(cause))?;
    let prefix = Prefix::parse(file.prefix.as_deref().unwrap_or(DEFAULT_PREFIX))
        .ok_or_else(|| Error::config_invalid(INVALID_PREFIX))?;
    Ok(Settings {
        prefix,
        mouse: file.mouse.unwrap_or(true),
        lists: Lists {
            codex: list(file.codex, (INVALID_CODEX_MODELS, INVALID_CODEX_EFFORTS))?,
            claude: list(file.claude, (INVALID_CLAUDE_MODELS, INVALID_CLAUDE_EFFORTS))?,
        },
    })
}

/// A provider table's lists: valid, distinct names, at most `MAX_LISTED` each;
/// `invalid` holds the table's messages for its models and its efforts.
fn list(file: Option<ProviderFile>, invalid: (&'static str, &'static str)) -> Result<List> {
    let Some(file) = file else {
        return Ok(List::default());
    };
    let valid = |names: &[String], check: fn(&str) -> bool| {
        names.len() <= MAX_LISTED
            && names.iter().all(|name| check(name))
            && names
                .iter()
                .enumerate()
                .all(|(index, name)| !names[..index].contains(name))
    };
    if !valid(&file.models, |name| {
        hive_input::validate::model(name).is_ok()
    }) {
        return Err(Error::config_invalid(invalid.0));
    }
    if !valid(&file.efforts, |name| {
        hive_input::validate::effort(name).is_ok()
    }) {
        return Err(Error::config_invalid(invalid.1));
    }
    Ok(List {
        models: file.models,
        efforts: file.efforts,
    })
}

/// What the key after the prefix does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Sidebar,
    NewTab,
    Split(super::layout::Split),
    Close,
    Tab(usize),
    Next,
    Previous,
    /// The other view of a split.
    Other,
    Attention,
    Spawn,
    Quit,
}

impl Action {
    fn from_key(key: &KeyEvent) -> Option<Self> {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return None;
        }
        let KeyCode::Char(character) = key.code else {
            return None;
        };
        Some(match character {
            's' => Self::Sidebar,
            't' => Self::NewTab,
            'v' => Self::Split(super::layout::Split::Columns),
            'h' => Self::Split(super::layout::Split::Rows),
            'w' => Self::Close,
            '1'..='9' => Self::Tab(usize::from(character as u8 - b'1')),
            'n' => Self::Next,
            'p' => Self::Previous,
            'o' => Self::Other,
            'a' => Self::Attention,
            'c' => Self::Spawn,
            'q' => Self::Quit,
            _ => return None,
        })
    }
}

/// Where a key goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Routed {
    /// To whatever has focus.
    Key,
    /// The prefix: the next key is an action.
    Pending,
    Action(Action),
    /// The prefix twice: the prefix key itself goes to the active view.
    Prefix,
    /// An unbound key after the prefix; it is never typed into a view.
    Dropped,
}

pub(super) struct Router {
    prefix: Prefix,
    pending: Option<Instant>,
}

impl Router {
    pub(super) fn new(prefix: Prefix) -> Self {
        Self {
            prefix,
            pending: None,
        }
    }

    pub(super) fn route(&mut self, key: &KeyEvent, now: Instant) -> Routed {
        if let Some(since) = self.pending.take()
            && now.duration_since(since) <= PREFIX_TIMEOUT
        {
            if self.prefix.matches(key) {
                return Routed::Prefix;
            }
            return Action::from_key(key).map_or(Routed::Dropped, Routed::Action);
        }
        if self.prefix.matches(key) {
            self.pending = Some(now);
            return Routed::Pending;
        }
        Routed::Key
    }

    /// Drop an expired prefix; `true` when the status line must change.
    pub(super) fn expire(&mut self, now: Instant) -> bool {
        let expired = self
            .pending
            .is_some_and(|since| now.duration_since(since) > PREFIX_TIMEOUT);
        if expired {
            self.pending = None;
        }
        expired
    }

    pub(super) fn pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn clear(&mut self) {
        self.pending = None;
    }

    pub(super) fn prefix(&self) -> Prefix {
        self.prefix
    }
}

#[cfg(test)]
mod tests;
