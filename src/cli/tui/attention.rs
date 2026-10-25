//! Pending approvals and questions across the tree, in the order this TUI
//! first saw them, and the bell and notification for new ones.
use super::sidebar::Agent;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use std::io::Write;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Entry {
    pub agent: String,
    pub approval: String,
}

/// What a key in the queue asks for.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Key {
    None,
    Open(String),
    Leave,
}

#[derive(Debug, Default)]
pub(super) struct Attention {
    pub entries: Vec<Entry>,
    pub selected: usize,
}

impl Attention {
    /// Track the agents' pending approvals; returns how many are new.
    pub(super) fn observe(&mut self, agents: &[Agent]) -> usize {
        let pending: Vec<Entry> = agents
            .iter()
            .filter(|agent| !agent.ended)
            .filter_map(|agent| {
                Some(Entry {
                    agent: agent.id.clone(),
                    approval: agent.approval.clone()?,
                })
            })
            .collect();
        self.entries.retain(|entry| pending.contains(entry));
        let before = self.entries.len();
        for entry in pending {
            if !self.entries.contains(&entry) {
                self.entries.push(entry);
            }
        }
        self.selected = self.selected.min(self.entries.len().saturating_sub(1));
        self.entries.len() - before
    }

    /// One wheel step: move the selection like ↑ or ↓.
    pub(super) fn step(&mut self, up: bool) {
        self.selected = if up {
            self.selected.saturating_sub(1)
        } else {
            (self.selected + 1).min(self.entries.len().saturating_sub(1))
        };
    }

    pub(super) fn key(&mut self, key: &KeyEvent) -> Key {
        match key.code {
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(self.entries.len().saturating_sub(1));
            }
            KeyCode::Enter => {
                if let Some(entry) = self.entries.get(self.selected) {
                    return Key::Open(entry.agent.clone());
                }
            }
            KeyCode::Esc => return Key::Leave,
            _ => {}
        }
        Key::None
    }
}

/// The bell and an OSC 9 notification with a fixed payload: never agent text.
pub(super) fn notification(waiting: usize) -> String {
    let text = if waiting == 1 {
        String::from("Hive: 1 agent needs you")
    } else {
        format!("Hive: {waiting} agents need you")
    };
    format!("\u{7}\u{1b}]9;{text}\u{7}")
}

/// Write the notification between frames.
pub(super) fn notify(sink: &mut impl Write, waiting: usize) -> std::io::Result<()> {
    sink.write_all(notification(waiting).as_bytes())?;
    sink.flush()
}

#[cfg(test)]
mod tests;
