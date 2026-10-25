//! Model and effort names listed in `tui.toml`, and the picker a view opens
//! for `/model` or `/effort` without an argument.
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthChar;

/// Most names one list may hold.
pub(in crate::cli) const MAX_LISTED: usize = 32;

/// What a picker or dialog field sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::cli) enum Kind {
    Model,
    Effort,
}

impl Kind {
    /// The agent option the name sets, which is also the view command.
    pub(in crate::cli) fn option(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Effort => "effort",
        }
    }
}

/// One provider's listed names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::cli) struct List {
    pub models: Vec<String>,
    pub efforts: Vec<String>,
}

impl List {
    pub(in crate::cli) fn names(&self, kind: Kind) -> &[String] {
        match kind {
            Kind::Model => &self.models,
            Kind::Effort => &self.efforts,
        }
    }
}

/// The names each provider lists; a view without `tui.toml` has none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::cli) struct Lists {
    pub codex: List,
    pub claude: List,
}

impl Lists {
    /// The names listed for a provider, as the daemon spells it.
    pub(in crate::cli) fn names(&self, provider: &str, kind: Kind) -> &[String] {
        match provider {
            "codex" => self.codex.names(kind),
            "claude" => self.claude.names(kind),
            _ => &[],
        }
    }
}

/// An open picker: the listed names, the selection and the agent's value.
#[derive(Debug)]
pub(super) struct Picker {
    pub kind: Kind,
    provider: String,
    names: Vec<String>,
    current: String,
    selected: usize,
}

impl Picker {
    /// A picker over `names`, starting on `current` when it is listed; none
    /// when nothing is listed.
    pub(super) fn new(kind: Kind, provider: &str, names: &[String], current: &str) -> Option<Self> {
        if names.is_empty() {
            return None;
        }
        Some(Self {
            kind,
            provider: provider.to_owned(),
            names: names.to_vec(),
            current: current.to_owned(),
            selected: names.iter().position(|name| name == current).unwrap_or(0),
        })
    }

    pub(super) fn up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub(super) fn down(&mut self) {
        self.selected = (self.selected + 1).min(self.names.len() - 1);
    }

    pub(super) fn chosen(&self) -> &str {
        &self.names[self.selected]
    }

    pub(super) fn select(&mut self, index: usize) {
        self.selected = index.min(self.names.len() - 1);
    }

    /// The card frame's title, such as `model (codex)`.
    pub(super) fn title(&self) -> String {
        format!("{} ({})", self.kind.option(), self.provider)
    }

    /// The first name shown when the card has `rows` text rows; the
    /// selection stays in view.
    fn top(&self, rows: usize) -> usize {
        self.selected.saturating_sub(rows.max(1) - 1)
    }

    /// The names that fit `rows` rows, each cut to `width` columns so rows
    /// map one to one to names. `✓` marks the agent's current value.
    pub(super) fn lines(&self, width: usize, rows: usize) -> Vec<Line<'static>> {
        self.names
            .iter()
            .enumerate()
            .skip(self.top(rows))
            .take(rows.max(1))
            .map(|(index, name)| {
                let chosen = index == self.selected;
                let marker = if chosen { "▸" } else { " " };
                let current = if *name == self.current { "✓" } else { " " };
                let style = if chosen {
                    Style::new().add_modifier(Modifier::REVERSED)
                } else {
                    Style::new()
                };
                Line::from(Span::styled(
                    cut(&format!("{marker}{current} {name}"), width),
                    style,
                ))
            })
            .collect()
    }

    /// The row under the picker's lines.
    pub(super) fn footer(&self) -> Line<'static> {
        Line::from(Span::styled(
            format!(
                " ↑↓ choose · Enter set · Esc close · /{} NAME sets any other",
                self.kind.option()
            ),
            Style::new().add_modifier(Modifier::DIM),
        ))
    }

    /// The name on line `line` of the picker drawn with `rows` text rows.
    pub(super) fn at(&self, line: usize, rows: usize) -> Option<usize> {
        if line >= rows.max(1) {
            return None;
        }
        let index = self.top(rows) + line;
        (index < self.names.len()).then_some(index)
    }
}

/// `text` cut to `width` columns.
fn cut(text: &str, width: usize) -> String {
    let mut used = 0;
    text.chars()
        .take_while(|character| {
            used += character.width().unwrap_or(0);
            used <= width
        })
        .collect()
}

#[cfg(test)]
mod tests;
