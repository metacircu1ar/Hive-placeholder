//! The mouse in `hivectl tui`: where the last frame drew each part, and what a
//! left click or wheel step there does. Each click takes its key's path.
use super::{Action, Console, Focus, Overlay, spawn};
use ratatui::{
    crossterm::event::{Event, MouseButton, MouseEvent, MouseEventKind},
    layout::{Position, Rect},
};
use std::time::Instant;

/// An agent row drawn in the sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Row {
    pub y: u16,
    pub agent: String,
    /// The column of the `▸`/`▾` marker of an agent with children.
    pub marker: Option<u16>,
}

/// Where the last frame drew each part.
#[derive(Debug, Default)]
pub(super) struct Hits {
    pub sidebar: Rect,
    pub rows: Vec<Row>,
    pub bar: Rect,
    /// Tab labels: first column, column after the last, tab index.
    pub tabs: Vec<(u16, u16, usize)>,
    /// Each view's area, border included.
    pub views: Vec<(String, Rect)>,
    pub overlay: Rect,
    /// Questions-list rows and the entry on each.
    pub entries: Vec<(u16, usize)>,
    /// Spawn-dialog rows, the column the line starts at, and its field.
    pub fields: Vec<(u16, u16, spawn::Field)>,
    pub button_row: Rect,
    /// Button-bar buttons: first column, column after the last, action.
    pub buttons: Vec<(u16, u16, Action)>,
}

impl Console<'_> {
    /// Handle one mouse event; `false` when it changed nothing.
    pub(super) fn mouse(&mut self, event: MouseEvent, now: Instant) -> bool {
        let point = Position::new(event.column, event.row);
        match event.kind {
            MouseEventKind::ScrollUp => self.wheel(point, true, event),
            MouseEventKind::ScrollDown => self.wheel(point, false, event),
            MouseEventKind::Down(MouseButton::Left) => {
                // A click ends a pending prefix and disarms a confirmation,
                // as a key does, unless it presses that button again.
                let armed = self.armed.take();
                self.router.clear();
                if self.hits.button_row.contains(point) {
                    let button = self
                        .hits
                        .buttons
                        .iter()
                        .find(|(start, end, _)| (*start..*end).contains(&point.x))
                        .map(|(_, _, action)| *action);
                    if let Some(action) = button {
                        self.action(action, armed, now);
                    }
                    return true;
                }
                self.click(point, event, now);
                true
            }
            _ => false,
        }
    }

    fn click(&mut self, point: Position, event: MouseEvent, now: Instant) {
        match &mut self.overlay {
            Overlay::Spawn(dialog) => {
                // Outside the dialog a click is ignored: typed text is kept.
                if let Some((_, x, field)) = self.hits.fields.iter().find(|(y, _, _)| *y == point.y)
                    && self.hits.overlay.contains(point)
                {
                    dialog.click(*field, point.x.saturating_sub(*x));
                }
                return;
            }
            Overlay::Attention => {
                if !self.hits.overlay.contains(point) {
                    self.overlay = Overlay::None;
                    return;
                }
                let entry = self
                    .hits
                    .entries
                    .iter()
                    .find(|(y, _)| *y == point.y)
                    .and_then(|(_, index)| self.attention.entries.get(*index).map(|e| (*index, e)))
                    .map(|(index, entry)| (index, entry.agent.clone()));
                if let Some((index, agent)) = entry {
                    self.attention.selected = index;
                    self.open_question(&agent, now);
                }
                return;
            }
            Overlay::None => {}
        }
        if self.hits.sidebar.contains(point) {
            let row = self.hits.rows.iter().find(|row| row.y == point.y).cloned();
            match row {
                Some(row) if row.marker == Some(point.x) => {
                    self.sidebar.toggle(&row.agent);
                    self.focus = Focus::Sidebar;
                }
                Some(row) => {
                    self.sidebar.pick(&row.agent);
                    self.open_picked(&row.agent, now);
                }
                None => self.focus = Focus::Sidebar,
            }
            return;
        }
        if self.hits.bar.contains(point) {
            let tab = self
                .hits
                .tabs
                .iter()
                .find(|(start, end, _)| (*start..*end).contains(&point.x))
                .map(|(_, _, index)| *index);
            if let Some(index) = tab {
                self.leave_sidebar();
                self.show(|layout| layout.select(index));
            }
            return;
        }
        let view = self
            .hits
            .views
            .iter()
            .find(|(_, area)| area.contains(point))
            .map(|(agent, _)| agent.clone());
        if let Some(agent) = view {
            self.leave_sidebar();
            self.layout.focus(&agent);
            self.focus = Focus::View;
            if let Some(view) = self.views.get_mut(&agent) {
                view.event(&Event::Mouse(event));
            }
        }
    }

    /// Leaving the sidebar by clicking elsewhere drops a pending `t`/`v`/`h`
    /// pick, as Esc does.
    fn leave_sidebar(&mut self) {
        if self.place.take().is_some() {
            self.status = self.hint();
        }
    }

    /// A wheel step scrolls what is under the pointer; views keep their focus.
    fn wheel(&mut self, point: Position, up: bool, event: MouseEvent) -> bool {
        match self.overlay {
            Overlay::Attention if self.hits.overlay.contains(point) => {
                self.attention.step(up);
                return true;
            }
            Overlay::None => {}
            _ => return false,
        }
        if self.hits.sidebar.contains(point) {
            self.sidebar.step(up);
            return true;
        }
        let view = self
            .hits
            .views
            .iter()
            .find(|(_, area)| area.contains(point))
            .map(|(agent, _)| agent.clone());
        view.and_then(|agent| self.views.get_mut(&agent))
            .is_some_and(|view| view.event(&Event::Mouse(event)))
    }
}
