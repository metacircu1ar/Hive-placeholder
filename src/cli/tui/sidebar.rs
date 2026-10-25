//! The agent tree: rows from `tree`, grouped by parent through retained
//! ancestry, with badges, collapsible subtrees and a fuzzy filter.
mod agent;

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    style::{Color, Modifier, Style},
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Ended agents kept with `--keep-ended` after they leave the tree.
const RETAINED_ENDED: usize = 1000;
/// Deeper agents are drawn at this indentation.
const MAX_DEPTH: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Agent {
    pub id: String,
    pub parent: Option<String>,
    pub role: Option<String>,
    /// The display name, which replaces the role-and-ID label.
    pub name: Option<String>,
    pub state: String,
    pub sandbox: String,
    pub outcome: String,
    pub waiting: Option<String>,
    pub approval: Option<String>,
    pub ended: bool,
}

impl Agent {}

pub(super) fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// Agent states, strongest first for roll-ups.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Badge {
    NeedsYou,
    Working,
    Quota,
    Idle,
    Done,
    Failed,
    Cancelled,
    Ended,
}

impl Badge {
    pub(super) fn symbol(self) -> &'static str {
        match self {
            Self::NeedsYou => "!",
            Self::Working => "●",
            Self::Quota => "◔",
            Self::Idle => "○",
            Self::Done => "✓",
            Self::Failed => "✗",
            Self::Cancelled | Self::Ended => "–",
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::NeedsYou => "needs you",
            Self::Working => "working",
            Self::Quota => "quota",
            Self::Idle => "idle",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Ended => "ended",
        }
    }

    pub(super) fn style(self) -> Style {
        match self {
            Self::NeedsYou => Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            Self::Working => Style::new().fg(Color::Green),
            Self::Quota => Style::new().fg(Color::Magenta),
            Self::Done => Style::new().fg(Color::Blue),
            Self::Failed => Style::new().fg(Color::Red),
            Self::Idle | Self::Cancelled | Self::Ended => Style::new().add_modifier(Modifier::DIM),
        }
    }

    /// Only these roll up to a collapsed parent.
    fn rolls_up(self) -> bool {
        matches!(self, Self::NeedsYou | Self::Working | Self::Quota)
    }
}

/// Agents in a tree reply, in its order.
pub(super) fn rows(reply: &Value) -> Vec<Agent> {
    let tree: crate::cli::views::Tree<'_> = crate::cli::views::read(reply);
    tree.agents
        .into_iter()
        .filter_map(|row| {
            Some(Agent {
                id: row.id?.to_owned(),
                parent: row.parent_id.map(str::to_owned),
                role: row.role.map(str::to_owned),
                name: row.name.map(str::to_owned),
                state: row.state.unwrap_or_default().to_owned(),
                sandbox: row.sandbox.unwrap_or_default().to_owned(),
                outcome: row.task_outcome.unwrap_or_default().to_owned(),
                waiting: row.waiting_reason.map(str::to_owned),
                approval: row.approval_id.map(str::to_owned),
                ended: row.ended,
            })
        })
        .collect()
}

/// One sidebar line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Item {
    pub id: String,
    pub depth: usize,
    pub badge: Badge,
    /// Descendants hidden under a collapsed agent.
    pub hidden: Option<usize>,
    pub children: bool,
}

/// What a sidebar key asks for.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Key {
    None,
    Open(String),
    Leave,
}

#[derive(Debug, Default)]
pub(super) struct Sidebar {
    agents: Vec<Agent>,
    /// Every parent link seen, so a live child of an ended agent stays under
    /// its nearest shown ancestor.
    parents: HashMap<String, Option<String>>,
    collapsed: HashSet<String>,
    pub filter: String,
    /// The selected agent, kept across updates.
    selected: Option<String>,
    pub truncated: bool,
    keep_ended: bool,
}

impl Sidebar {
    pub(super) fn new(keep_ended: bool) -> Self {
        Self {
            keep_ended,
            ..Self::default()
        }
    }

    pub(super) fn agents(&self) -> &[Agent] {
        &self.agents
    }

    pub(super) fn agent(&self, id: &str) -> Option<&Agent> {
        self.agents.iter().find(|agent| agent.id == id)
    }

    /// Replace the rows. With `--keep-ended`, agents that left the tree stay as
    /// ended, with the outcome their open view saw, if any.
    pub(super) fn update(
        &mut self,
        rows: Vec<Agent>,
        truncated: bool,
        outcome: impl Fn(&str) -> Option<String>,
    ) {
        for row in &rows {
            self.parents.insert(row.id.clone(), row.parent.clone());
        }
        let mut agents: Vec<Agent> = rows
            .into_iter()
            .filter(|agent| self.keep_ended || !agent.ended)
            .collect();
        if self.keep_ended {
            // A truncated tree omits live agents too: only agents already
            // known to have ended are kept then.
            let present: HashSet<&str> = agents.iter().map(|agent| agent.id.as_str()).collect();
            let missing = self
                .agents
                .iter()
                .filter(|agent| !present.contains(agent.id.as_str()));
            // Agents retained earlier come first, newly ended ones last, so the
            // cap drops the oldest.
            let (mut gone, just_ended): (Vec<Agent>, Vec<Agent>) = missing
                .filter(|agent| agent.ended || !truncated)
                .cloned()
                .partition(|agent| agent.ended);
            gone.extend(ended_now(just_ended, &outcome));
            let skip = gone.len().saturating_sub(RETAINED_ENDED);
            agents.extend(gone.into_iter().skip(skip));
        }
        self.agents = agents;
        self.truncated = truncated;
        self.prune();
    }

    /// Keep only links of shown agents and their ancestors.
    fn prune(&mut self) {
        let mut keep: HashSet<String> = HashSet::new();
        for agent in &self.agents {
            let mut next = Some(agent.id.clone());
            while let Some(id) = next {
                if !keep.insert(id.clone()) {
                    break;
                }
                next = self.parents.get(&id).cloned().flatten();
            }
        }
        self.parents.retain(|id, _| keep.contains(id));
        self.collapsed.retain(|id| keep.contains(id));
    }

    /// The nearest ancestor that is shown.
    fn shown_parent(&self, id: &str, shown: &HashSet<&str>) -> Option<String> {
        let mut seen = HashSet::new();
        let mut next = self.parents.get(id).cloned().flatten();
        while let Some(parent) = next {
            if shown.contains(parent.as_str()) {
                return Some(parent);
            }
            if !seen.insert(parent.clone()) {
                return None;
            }
            next = self.parents.get(&parent).cloned().flatten();
        }
        None
    }

    /// The lines to draw: the tree, or matching agents while filtering.
    pub(super) fn items(&self) -> Vec<Item> {
        if !self.filter.is_empty() {
            let query = self.filter.to_lowercase();
            return self
                .agents
                .iter()
                .filter(|agent| fuzzy(&query, &haystack(agent)))
                .map(|agent| Item {
                    id: agent.id.clone(),
                    depth: 0,
                    badge: agent.badge(),
                    hidden: None,
                    children: false,
                })
                .collect();
        }
        let shown: HashSet<&str> = self.agents.iter().map(|agent| agent.id.as_str()).collect();
        let mut children: HashMap<Option<String>, Vec<&Agent>> = HashMap::new();
        for agent in &self.agents {
            children
                .entry(self.shown_parent(&agent.id, &shown))
                .or_default()
                .push(agent);
        }
        let mut items = Vec::new();
        let mut visited = HashSet::new();
        let mut stack: Vec<(&Agent, usize)> = children
            .get(&None)
            .map(|roots| roots.iter().rev().map(|agent| (*agent, 0)).collect())
            .unwrap_or_default();
        while let Some((agent, depth)) = stack.pop() {
            if !visited.insert(agent.id.as_str()) {
                continue;
            }
            let below = children.get(&Some(agent.id.clone()));
            let collapsed = self.collapsed.contains(&agent.id);
            let mut item = Item {
                id: agent.id.clone(),
                depth: depth.min(MAX_DEPTH),
                badge: agent.badge(),
                hidden: None,
                children: below.is_some(),
            };
            if collapsed && below.is_some() {
                let (count, strongest) = descendants(&agent.id, &children);
                item.hidden = Some(count);
                if let Some(strongest) = strongest.filter(|badge| *badge < item.badge) {
                    item.badge = strongest;
                }
            } else if let Some(below) = below {
                stack.extend(below.iter().rev().map(|child| (*child, depth + 1)));
            }
            items.push(item);
        }
        items
    }

    fn selected_index(&self, items: &[Item]) -> usize {
        self.selected
            .as_ref()
            .and_then(|id| items.iter().position(|item| &item.id == id))
            .unwrap_or(0)
    }

    /// The selected line, for drawing.
    pub(super) fn selection(&self, items: &[Item]) -> Option<usize> {
        (!items.is_empty()).then(|| self.selected_index(items))
    }

    pub(super) fn key(&mut self, key: &KeyEvent) -> Key {
        let items = self.items();
        let index = self.selected_index(&items);
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Up => self.select(&items, index.saturating_sub(1)),
            KeyCode::Down => self.select(&items, index + 1),
            KeyCode::Home => self.select(&items, 0),
            KeyCode::End => self.select(&items, items.len().saturating_sub(1)),
            KeyCode::Enter => {
                if let Some(item) = items.get(index) {
                    // The opened agent stays selected in the whole tree.
                    self.selected = Some(item.id.clone());
                    self.filter.clear();
                    return Key::Open(item.id.clone());
                }
            }
            KeyCode::Left => {
                if let Some(item) = items.get(index) {
                    if item.children && item.hidden.is_none() {
                        self.collapsed.insert(item.id.clone());
                    } else if let Some(parent) = (0..index)
                        .rev()
                        .find(|above| items[*above].depth < item.depth)
                    {
                        self.select(&items, parent);
                    }
                }
            }
            KeyCode::Right => {
                if let Some(item) = items.get(index) {
                    self.collapsed.remove(&item.id);
                }
            }
            KeyCode::Esc => {
                self.filter.clear();
                return Key::Leave;
            }
            KeyCode::Backspace => {
                self.filter.pop();
            }
            KeyCode::Char(character) if !control => self.filter.push(character),
            _ => {}
        }
        Key::None
    }

    /// A click on an agent's row: select it and clear the filter, as Enter does.
    pub(super) fn pick(&mut self, id: &str) {
        self.selected = Some(id.to_owned());
        self.filter.clear();
    }

    /// Collapse or expand an agent's subtree.
    pub(super) fn toggle(&mut self, id: &str) {
        if !self.collapsed.remove(id) {
            self.collapsed.insert(id.to_owned());
        }
    }

    /// One wheel step: move the selection like ↑ or ↓.
    pub(super) fn step(&mut self, up: bool) {
        let items = self.items();
        let index = self.selected_index(&items);
        let next = if up {
            index.saturating_sub(1)
        } else {
            index + 1
        };
        self.select(&items, next);
    }

    fn select(&mut self, items: &[Item], index: usize) {
        if let Some(item) = items.get(index.min(items.len().saturating_sub(1))) {
            self.selected = Some(item.id.clone());
        }
    }

    pub(super) fn paste(&mut self, text: &str) {
        self.filter
            .extend(text.chars().filter(|character| !character.is_control()));
    }
}

/// Agents that just left the live tree, marked ended with the outcome their
/// view saw, if any.
fn ended_now(agents: Vec<Agent>, outcome: impl Fn(&str) -> Option<String>) -> Vec<Agent> {
    agents
        .into_iter()
        .map(|mut agent| {
            agent.ended = true;
            agent.approval = None;
            if let Some(seen) = outcome(&agent.id) {
                agent.outcome = seen;
            }
            agent
        })
        .collect()
}

/// How many agents sit below `id`, and the strongest badge that rolls up.
fn descendants(
    id: &str,
    children: &HashMap<Option<String>, Vec<&Agent>>,
) -> (usize, Option<Badge>) {
    let mut count = 0;
    let mut strongest: Option<Badge> = None;
    let mut seen = HashSet::new();
    let mut stack = vec![id.to_owned()];
    while let Some(next) = stack.pop() {
        for child in children.get(&Some(next)).into_iter().flatten() {
            if !seen.insert(child.id.clone()) {
                continue;
            }
            count += 1;
            let badge = child.badge();
            if badge.rolls_up() && strongest.is_none_or(|best| badge < best) {
                strongest = Some(badge);
            }
            stack.push(child.id.clone());
        }
    }
    (count, strongest)
}

fn haystack(agent: &Agent) -> String {
    format!(
        "{} {} {} {} {}",
        agent.name.as_deref().unwrap_or_default(),
        agent.label(),
        agent.short(),
        agent.state,
        agent.badge().name()
    )
    .to_lowercase()
}

/// The query's characters appear in order in the text.
pub(super) fn fuzzy(query: &str, text: &str) -> bool {
    let mut characters = text.chars();
    query
        .chars()
        .filter(|character| !character.is_whitespace())
        .all(|wanted| characters.any(|character| character == wanted))
}

#[cfg(test)]
mod tests;
