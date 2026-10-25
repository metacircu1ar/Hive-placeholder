//! Tabs of agent views: each tab shows one view or a split of two.
use ratatui::layout::{Constraint, Layout as Areas, Rect};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Split {
    /// Left and right.
    Columns,
    /// Top and bottom.
    Rows,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Tab {
    /// One or two agent IDs.
    pub views: Vec<String>,
    pub split: Split,
    pub active: usize,
}

/// Where a newly opened view goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Place {
    /// A new tab.
    Tab,
    /// Beside the current tab's only view.
    Split(Split),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Opened {
    /// The agent already had a view; it is now active.
    Focused,
    /// A new view, replacing the least recently used one when there was no room.
    New {
        replaced: Option<String>,
    },
    Refused(&'static str),
}

/// What limits a new view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Room {
    Free,
    /// At the cap: an idle view may be replaced.
    Replace,
    /// At the cap with nothing to replace yet.
    Full,
}

#[derive(Debug, Default)]
pub(super) struct Layout {
    pub tabs: Vec<Tab>,
    pub current: usize,
    /// Last use of each open view, for replacement.
    used: HashMap<String, u64>,
    clock: u64,
}

impl Layout {
    pub(super) fn contains(&self, agent: &str) -> bool {
        self.used.contains_key(agent)
    }

    pub(super) fn count(&self) -> usize {
        self.used.len()
    }

    /// The active view of the current tab.
    pub(super) fn active(&self) -> Option<&str> {
        let tab = self.tabs.get(self.current)?;
        tab.views.get(tab.active).map(String::as_str)
    }

    fn touch(&mut self, agent: &str) {
        self.clock += 1;
        self.used.insert(agent.to_owned(), self.clock);
    }

    /// Open a view of `agent`, or focus the one it has. Without room, the least
    /// recently used view that is not busy is replaced.
    pub(super) fn open(
        &mut self,
        agent: &str,
        place: Place,
        room: Room,
        busy: impl Fn(&str) -> bool,
    ) -> Opened {
        if self.contains(agent) {
            self.focus(agent);
            return Opened::Focused;
        }
        if matches!(place, Place::Split(_))
            && self
                .tabs
                .get(self.current)
                .is_some_and(|tab| tab.views.len() > 1)
        {
            return Opened::Refused("this tab already shows two views; close one first");
        }
        let replaced = match room {
            Room::Free => None,
            Room::Full => {
                return Opened::Refused(
                    "views closed in the last 25 seconds still hold daemon slots; retry shortly",
                );
            }
            Room::Replace => {
                // A split keeps the tab it splits: its view is never replaced.
                let kept: &[String] = match (place, self.tabs.get(self.current)) {
                    (Place::Split(_), Some(tab)) => &tab.views,
                    _ => &[],
                };
                let idle = self
                    .used
                    .iter()
                    .filter(|(view, _)| !busy(view) && !kept.contains(view))
                    .min_by_key(|(_, used)| **used)
                    .map(|(view, _)| view.clone());
                let Some(idle) = idle else {
                    return Opened::Refused(if kept.is_empty() {
                        "every view has a write in flight; close one first"
                    } else {
                        "no view outside this tab can be replaced; close one first"
                    });
                };
                self.close(&idle);
                Some(idle)
            }
        };
        let split = match place {
            Place::Split(split) => self
                .tabs
                .get_mut(self.current)
                .filter(|tab| tab.views.len() == 1)
                .map(|tab| (tab, split)),
            Place::Tab => None,
        };
        if let Some((tab, split)) = split {
            tab.views.push(agent.to_owned());
            tab.split = split;
            tab.active = 1;
        } else {
            self.tabs.push(Tab {
                views: vec![agent.to_owned()],
                split: Split::Columns,
                active: 0,
            });
            self.current = self.tabs.len() - 1;
        }
        self.touch(agent);
        Opened::New { replaced }
    }

    /// Close an agent's view; `false` when it had none.
    pub(super) fn close(&mut self, agent: &str) -> bool {
        let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.views.iter().any(|view| view == agent))
        else {
            return false;
        };
        self.used.remove(agent);
        let tab = &mut self.tabs[index];
        tab.views.retain(|view| view != agent);
        tab.active = tab.active.min(tab.views.len().saturating_sub(1));
        if tab.views.is_empty() {
            self.tabs.remove(index);
            if self.current > index || self.current >= self.tabs.len() {
                self.current = self.current.saturating_sub(1);
            }
        }
        true
    }

    /// Make an open view active.
    pub(super) fn focus(&mut self, agent: &str) {
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            if let Some(position) = tab.views.iter().position(|view| view == agent) {
                tab.active = position;
                self.current = index;
                self.touch(agent);
                return;
            }
        }
    }

    pub(super) fn select(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.current = index;
            self.touch_active();
        }
    }

    pub(super) fn next(&mut self) {
        if !self.tabs.is_empty() {
            self.current = (self.current + 1) % self.tabs.len();
            self.touch_active();
        }
    }

    pub(super) fn previous(&mut self) {
        if !self.tabs.is_empty() {
            self.current = (self.current + self.tabs.len() - 1) % self.tabs.len();
            self.touch_active();
        }
    }

    /// Switch to the other view of a split.
    pub(super) fn other(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.current)
            && tab.views.len() == 2
        {
            tab.active = 1 - tab.active;
            self.touch_active();
        }
    }

    fn touch_active(&mut self) {
        if let Some(agent) = self.active().map(str::to_owned) {
            self.touch(&agent);
        }
    }

    /// The current tab's views, their areas and which one is active.
    pub(super) fn areas(&self, area: Rect) -> Vec<(String, Rect, bool)> {
        let Some(tab) = self.tabs.get(self.current) else {
            return Vec::new();
        };
        let areas: Vec<Rect> = if tab.views.len() == 2 {
            let halves = [Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)];
            match tab.split {
                Split::Columns => Areas::horizontal(halves).split(area).to_vec(),
                Split::Rows => Areas::vertical(halves).split(area).to_vec(),
            }
        } else {
            vec![area]
        };
        tab.views
            .iter()
            .zip(areas)
            .enumerate()
            .map(|(index, (view, area))| (view.clone(), area, index == tab.active))
            .collect()
    }
}

#[cfg(test)]
mod tests;
