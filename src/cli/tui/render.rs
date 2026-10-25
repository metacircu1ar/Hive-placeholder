//! Render for the Console view.
use super::{
    Action, Areas, Block, Clear, Color, Console, Constraint, Focus, Frame, Line, Modifier, Overlay,
    Paragraph, Rect, Span, Style, centered, draw_spawn, layout, pointer, sidebar,
};
use std::fmt::Write as _;
use unicode_width::UnicodeWidthStr;

impl Console<'_> {
    pub(super) fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let width = (area.width / 4).clamp(20, 36).min(area.width / 2);
        let [side, main] =
            Areas::horizontal([Constraint::Length(width), Constraint::Min(10)]).areas(area);
        let [bar, body, status, buttons] = Areas::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(main);
        let mut hits = pointer::Hits {
            sidebar: side,
            rows: self.draw_sidebar(frame, side),
            bar,
            ..pointer::Hits::default()
        };
        let (tab_line, tabs) = self.tab_line();
        hits.tabs = tabs
            .into_iter()
            .map(|(start, end, index)| (bar.x + start, bar.x + end, index))
            .collect();
        frame.render_widget(Paragraph::new(tab_line), bar);
        let views_focused = self.focus == Focus::View && matches!(self.overlay, Overlay::None);
        let areas = self.layout.areas(body);
        if areas.is_empty() {
            let prefix = self.prefix();
            frame.render_widget(
                Paragraph::new(format!(
                    "No agent view is open. Pick an agent in the sidebar and press Enter, or \
                        {prefix} c to spawn one."
                ))
                .style(Style::new().add_modifier(Modifier::DIM))
                .wrap(ratatui::widgets::Wrap { trim: true }),
                body,
            );
        }
        for (agent, area, active) in areas {
            let title = self.title(&agent);
            let style = if active && views_focused {
                Style::new().fg(Color::Yellow)
            } else {
                Style::new().add_modifier(Modifier::DIM)
            };
            let block = Block::bordered().title(title).border_style(style);
            let inner = block.inner(area);
            frame.render_widget(block, area);
            if let Some(view) = self.views.get_mut(&agent) {
                view.draw(frame, inner, active && views_focused);
            }
            hits.views.push((agent, area));
        }
        frame.render_widget(Paragraph::new(self.status_line()), status);
        let (button_line, ranges) = self.button_bar();
        frame.render_widget(Paragraph::new(button_line), buttons);
        hits.button_row = buttons;
        hits.buttons = ranges
            .into_iter()
            .map(|(start, end, action)| (buttons.x + start, buttons.x + end, action))
            .collect();
        match &self.overlay {
            Overlay::None => {}
            Overlay::Attention => {
                (hits.overlay, hits.entries) = self.draw_attention(frame, body);
            }
            Overlay::Spawn(dialog) => {
                (hits.overlay, hits.fields) = draw_spawn(frame, body, dialog);
            }
        }
        self.hits = hits;
    }

    pub(super) fn title(&self, agent: &str) -> String {
        match self.sidebar.agent(agent) {
            Some(row) => format!(" {} ", row.display()),
            None => format!(" {} ", sidebar::short(agent)),
        }
    }

    /// The tab bar, and each label's columns (start, end) and tab index.
    pub(super) fn tab_line(&self) -> (Line<'static>, Vec<(u16, u16, usize)>) {
        let mut spans = Vec::new();
        let mut ranges = Vec::new();
        let mut column: u16 = 0;
        for (index, tab) in self.layout.tabs.iter().enumerate() {
            let names: Vec<&str> = tab
                .views
                .iter()
                .map(|agent| match self.sidebar.agent(agent) {
                    Some(row) => row.tab_label(),
                    None => sidebar::short(agent),
                })
                .collect();
            let style = if index == self.layout.current {
                Style::new().add_modifier(Modifier::REVERSED)
            } else {
                Style::new().add_modifier(Modifier::DIM)
            };
            let label = format!(" {}:{} ", index + 1, names.join("|"));
            let width = u16::try_from(label.width()).unwrap_or(u16::MAX);
            ranges.push((column, column.saturating_add(width), index));
            column = column.saturating_add(width).saturating_add(1);
            spans.push(Span::styled(label, style));
            spans.push(Span::raw(" "));
        }
        (Line::from(spans), ranges)
    }

    /// The clickable buttons under the status line, each one a prefix
    /// action, and their columns (start, end).
    pub(super) fn button_bar(&self) -> (Line<'static>, Vec<(u16, u16, Action)>) {
        let buttons = [
            (" Sidebar ", Action::Sidebar),
            (" Split ", Action::Split(layout::Split::Columns)),
            (" Close ", Action::Close),
            (" Questions ", Action::Attention),
            (" Spawn ", Action::Spawn),
            (" Quit ", Action::Quit),
        ];
        let mut spans = Vec::new();
        let mut ranges = Vec::new();
        let mut column: u16 = 0;
        for (label, action) in buttons {
            let width = u16::try_from(label.width()).unwrap_or(u16::MAX);
            ranges.push((column, column.saturating_add(width), action));
            column = column.saturating_add(width).saturating_add(1);
            spans.push(Span::styled(
                label,
                Style::new().add_modifier(Modifier::REVERSED),
            ));
            spans.push(Span::raw(" "));
        }
        // The pending prefix lists every key; this only says how to get there.
        spans.push(Span::styled(
            format!("{}: more keys", self.prefix()),
            Style::new().add_modifier(Modifier::DIM),
        ));
        (Line::from(spans), ranges)
    }

    pub(super) fn status_line(&self) -> Line<'static> {
        let dim = Style::new().add_modifier(Modifier::DIM);
        if self.router.pending() {
            return Line::from(Span::styled(
                format!(
                    "{} … s sidebar  t tab  v|h split  w close  1-9 n p o tabs  a attention  c \
                        spawn  q quit",
                    self.prefix()
                ),
                Style::new().fg(Color::Yellow),
            ));
        }
        let mut spans = Vec::new();
        let needing = self.attention.entries.len();
        if needing > 0 {
            spans.push(Span::styled(
                format!("! {needing} need you "),
                Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ));
        }
        let waiting =
            self.views.values().filter(|view| view.waiting()).count() + usize::from(self.waiting);
        if waiting > 0 {
            let sidebar = if self.waiting {
                " (sidebar included)"
            } else {
                ""
            };
            spans.push(Span::styled(
                format!("{waiting} waiting for a viewer slot{sidebar} "),
                Style::new().fg(Color::Magenta),
            ));
        }
        if !self.connected {
            spans.push(Span::styled(
                "disconnected, retrying ",
                Style::new().fg(Color::Red),
            ));
        }
        spans.push(Span::styled(self.status.clone(), dim));
        Line::from(spans)
    }

    /// Draw the sidebar; returns each agent row drawn, with the column of its
    /// collapse marker when it has children.
    pub(super) fn draw_sidebar(&self, frame: &mut Frame<'_>, area: Rect) -> Vec<pointer::Row> {
        let focused = self.focus == Focus::Sidebar && matches!(self.overlay, Overlay::None);
        let mut title = format!(" agents {} ", self.sidebar.agents().len());
        if self.sidebar.truncated {
            title.push_str("(truncated) ");
        }
        let style = if focused {
            Style::new().fg(Color::Yellow)
        } else {
            Style::new().add_modifier(Modifier::DIM)
        };
        let block = Block::bordered().title(title).border_style(style);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let items = self.sidebar.items();
        let selected = self.sidebar.selection(&items);
        let mut lines = Vec::new();
        if !self.sidebar.filter.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("/{}", self.sidebar.filter),
                Style::new().fg(Color::Cyan),
            )));
        }
        let height = usize::from(inner.height).saturating_sub(lines.len()).max(1);
        let skip = selected.map_or(0, |selected| (selected + 1).saturating_sub(height));
        let mut rows = Vec::new();
        for (index, item) in items.iter().enumerate().skip(skip).take(height) {
            let Some(agent) = self.sidebar.agent(&item.id) else {
                continue;
            };
            let depth = u16::try_from(item.depth).unwrap_or(u16::MAX);
            rows.push(pointer::Row {
                y: inner
                    .y
                    .saturating_add(u16::try_from(lines.len()).unwrap_or(u16::MAX)),
                agent: item.id.clone(),
                marker: (item.children || item.hidden.is_some())
                    .then(|| inner.x.saturating_add(depth.saturating_mul(2))),
            });
            let marker = match (item.hidden, item.children) {
                (Some(_), _) => "▸",
                (None, true) => "▾",
                (None, false) => " ",
            };
            let mut text = format!(
                "{}{marker}{} {}",
                "  ".repeat(item.depth),
                item.badge.symbol(),
                agent.display()
            );
            if let Some(hidden) = item.hidden {
                // Writing to a String cannot fail.
                let _ = write!(text, " +{hidden}");
            }
            let mut style = item.badge.style();
            if Some(index) == selected {
                style = if focused {
                    style.add_modifier(Modifier::REVERSED)
                } else {
                    style.add_modifier(Modifier::BOLD)
                };
            }
            lines.push(agent.sidebar_line(text, style));
        }
        if items.is_empty() {
            let text = if self.connected {
                "no live agents"
            } else {
                "connecting…"
            };
            lines.push(Line::from(Span::styled(
                text,
                Style::new().add_modifier(Modifier::DIM),
            )));
        }
        frame.render_widget(Paragraph::new(lines), inner);
        rows
    }

    /// Draw the questions list; returns its area and each entry's row.
    pub(super) fn draw_attention(
        &self,
        frame: &mut Frame<'_>,
        body: Rect,
    ) -> (Rect, Vec<(u16, usize)>) {
        let entries = &self.attention.entries;
        let height = u16::try_from(entries.len().max(1) + 4).unwrap_or(u16::MAX);
        let area = centered(body, 64, height);
        frame.render_widget(Clear, area);
        let block = Block::bordered()
            .title(format!(" needs you ({}) ", entries.len()))
            .border_style(Style::new().fg(Color::Yellow));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let mut lines = Vec::new();
        let mut rows = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            rows.push((
                inner
                    .y
                    .saturating_add(u16::try_from(index).unwrap_or(u16::MAX)),
                index,
            ));
            let label = self.sidebar.agent(&entry.agent).map_or_else(
                || format!("agent {}", sidebar::short(&entry.agent)),
                sidebar::Agent::display,
            );
            let mut style = Style::new();
            if index == self.attention.selected {
                style = style.add_modifier(Modifier::REVERSED);
            }
            lines.push(Line::from(Span::styled(format!("! {label}"), style)));
        }
        if entries.is_empty() {
            lines.push(Line::from("Nothing is waiting for you."));
        }
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "Enter opens the agent at its question · Esc closes",
            Style::new().add_modifier(Modifier::DIM),
        )));
        frame.render_widget(Paragraph::new(lines), inner);
        (area, rows)
    }
}
