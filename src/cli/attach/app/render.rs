//! Render for the App view.
use super::{App, Frame, Link, Panel, Paragraph, Rect, Screen, Wrap};

impl App {
    /// The frame to draw in `area`. An open picker takes the question card's
    /// place; either fits the card's width and rows inside its border.
    pub(in crate::cli::attach) fn screen(&self, active: bool, area: Rect) -> Option<Screen<'_>> {
        let model = self.model.as_ref()?;
        let width = usize::from(area.width.saturating_sub(2));
        let rows = super::super::view::card_rows(area.height);
        let card = match (&self.picker, &self.card) {
            (Some(picker), _) => Some(Panel {
                title: picker.title(),
                lines: picker.lines(width, rows),
                footer: picker.footer(),
            }),
            (None, Some(card)) => Some(Panel {
                title: String::from("question"),
                lines: card.lines(width, rows),
                footer: card.buttons().0,
            }),
            (None, None) => None,
        };
        Some(Screen {
            model,
            scroll: self.scroll,
            input: (!self.mode.read_only).then_some((self.input.as_str(), self.cursor)),
            focus: self.focus,
            card,
            status: &self.status,
            completion: self.slash_hint(),
            connected: self.link != Link::Disconnected,
            active,
        })
    }

    /// Draw the view into `area`, the only drawing path for `hivectl attach` and
    /// `hivectl tui`. Only the active view places the terminal cursor.
    pub(in crate::cli::attach) fn draw_into(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        active: bool,
    ) {
        let shown = self
            .screen(active, area)
            .map(|screen| super::super::view::draw(frame, area, &screen));
        if let Some(shown) = shown {
            self.shown = shown;
            self.clamp_scroll();
        } else {
            let text = format!(
                "Connecting to {}: {}\n{}",
                self.target,
                self.status,
                self.hint()
            );
            frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), area);
        }
    }
}
