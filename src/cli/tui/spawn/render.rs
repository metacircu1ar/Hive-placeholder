//! Render for the Dialog view.
use super::{
    Color, Dialog, FIELDS, Field, Kind, LABEL, Line, MIN_TYPING, Modifier, Rendered, SANDBOXES,
    Span, Style, UnicodeWidthStr, choice_text, scrolled, text_or,
};

impl Dialog {
    /// The shown value of a one-line field.
    pub(super) fn value(&self, field: Field) -> String {
        match field {
            Field::Role => choice_text(self.role.and_then(|i| self.roles.get(i)), "(none)"),
            Field::Login => choice_text(
                self.login.and_then(|i| self.logins.get(i)),
                "(role or default)",
            ),
            Field::Name => text_or(&self.name, "(none)"),
            Field::Model => text_or(&self.model, "(role or login default)"),
            Field::Effort => text_or(&self.effort, "(role or model default)"),
            Field::Sandbox => format!(
                "‹ {} ›",
                self.sandbox
                    .and_then(|index| SANDBOXES.get(index))
                    .copied()
                    .unwrap_or("(role or default)")
            ),
            Field::Directory => self.directory.clone(),
            Field::Body => String::new(),
        }
    }

    /// A one-line field: its label, its value and, when it is focused and
    /// listed, how many names ← and → cycle through while the typed text
    /// keeps room enough to edit.
    pub(super) fn field_line(
        &self,
        field: Field,
        label: Span<'static>,
        width: usize,
        rendered: &mut Rendered,
    ) {
        let focused = field == self.focus;
        let (typed, kind) = match field {
            Field::Name => (Some(&self.name), None),
            Field::Model => (Some(&self.model), Some(Kind::Model)),
            Field::Effort => (Some(&self.effort), Some(Kind::Effort)),
            Field::Directory => (Some(&self.directory), None),
            Field::Role | Field::Login | Field::Sandbox | Field::Body => (None, None),
        };
        let room = width.saturating_sub(LABEL);
        let hint = kind
            .map(|kind| self.listed(kind).len())
            .filter(|count| focused && *count > 0)
            .map(|count| format!("  ←/→ {count} listed"))
            .filter(|hint| room >= hint.width() + MIN_TYPING);
        let room = room - hint.as_ref().map_or(0, |hint| hint.width());
        let value = match typed {
            Some(typed) if focused && !typed.is_empty() => {
                let (shown, column) = scrolled(typed, room);
                rendered.cursor = Some((rendered.lines.len(), LABEL + column));
                shown
            }
            Some(_) if focused => {
                rendered.cursor = Some((rendered.lines.len(), LABEL));
                self.value(field)
            }
            _ => self.value(field),
        };
        let mut spans = vec![label, Span::raw(value)];
        if let Some(hint) = hint {
            spans.push(Span::styled(hint, Style::new().add_modifier(Modifier::DIM)));
        }
        rendered.push(Line::from(spans), Some(field));
    }

    /// The dialog's lines for `width` columns, the field on each line, and
    /// where the cursor goes. Lines are cut, not wrapped, so they map one to
    /// one to rows; the focused text scrolls to keep its cursor in view.
    pub(in crate::cli::tui) fn lines(&self, width: usize) -> Rendered {
        let dim = Style::new().add_modifier(Modifier::DIM);
        let mut rendered = Rendered::default();
        for field in FIELDS {
            let focused = field == self.focus;
            let label = Span::styled(
                format!("{:<LABEL$}", field.label()),
                if focused {
                    Style::new().add_modifier(Modifier::BOLD)
                } else {
                    dim
                },
            );
            if field == Field::Body {
                rendered.push(Line::from(label), Some(field));
                let body: Vec<&str> = self.body.split('\n').collect();
                for (index, line) in body.iter().enumerate() {
                    if focused && index + 1 == body.len() {
                        let (shown, column) = scrolled(line, width.saturating_sub(2));
                        rendered.cursor = Some((rendered.lines.len(), 2 + column));
                        rendered.push(Line::from(format!("  {shown}")), Some(field));
                    } else {
                        rendered.push(Line::from(format!("  {line}")), Some(field));
                    }
                }
            } else {
                self.field_line(field, label, width, &mut rendered);
            }
            if let Some((_, problem)) = self.error.as_ref().filter(|(at, _)| *at == field) {
                rendered.push(
                    Line::from(Span::styled(
                        format!("{:LABEL$}{problem}", ""),
                        Style::new().fg(Color::Red),
                    )),
                    Some(field),
                );
            }
        }
        rendered.push(Line::default(), None);
        let provider = self.provider().unwrap_or("from the config");
        rendered.push(
            Line::from(Span::styled(format!("provider: {provider}"), dim)),
            None,
        );
        if self.scoped {
            rendered.push(
                Line::from(Span::styled(
                    "The agent starts outside this TUI's scope; reopen it with hivectl attach ID.",
                    dim,
                )),
                None,
            );
        }
        rendered.push(Line::from(Span::styled(self.footer.clone(), dim)), None);
        rendered
    }
}
