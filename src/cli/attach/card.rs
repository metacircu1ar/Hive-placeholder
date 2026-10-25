//! The pending question as a card: a simple approval, or a form built from its
//! response schema and validated with the daemon's own check before `respond`.
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use serde_json::{Map, Value};
use unicode_width::UnicodeWidthStr;

/// The answer buttons on every card: the action and its label.
const BUTTONS: [(&str, &str); 3] = [
    ("accept", " Accept "),
    ("decline", " Decline "),
    ("cancel", " Cancel "),
];

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    Text,
    Integer,
    Number,
    /// A JSON array editor; the shared schema validates choices and item bounds.
    Array,
    Boolean,
    Choice(Vec<String>),
}

#[derive(Debug)]
struct Field {
    name: String,
    kind: Kind,
    required: bool,
    value: String,
}

#[derive(Debug)]
pub(super) struct Card {
    pub id: String,
    title: Option<String>,
    message: String,
    destructive: bool,
    schema: Option<Value>,
    fields: Vec<Field>,
    selected: usize,
}

impl Card {
    /// A card for an open approval; `None` for `Null` or a closed one.
    pub(super) fn from_detail(detail: &Value) -> Option<Self> {
        let detail: crate::cli::views::Approval<'_> = crate::cli::views::read(detail);
        let id = detail.id?.to_owned();
        if detail.state.is_some_and(|state| state != "pending") {
            return None;
        }
        let schema = detail.schema;
        let fields = schema.as_ref().map(fields).unwrap_or_default();
        Some(Self {
            id,
            title: detail.title.map(str::to_owned),
            message: detail.message.unwrap_or_default().to_owned(),
            destructive: detail.destructive,
            schema,
            fields,
            selected: 0,
        })
    }

    /// A plain approval without a form: single keys may answer it.
    pub(super) fn simple(&self) -> bool {
        self.fields.is_empty()
    }

    /// Pasted text goes into the selected text or number field.
    pub(super) fn paste(&mut self, text: &str) {
        if let Some(field) = self.fields.get_mut(self.selected)
            && matches!(
                field.kind,
                Kind::Text | Kind::Integer | Kind::Number | Kind::Array
            )
        {
            field.value.push_str(text);
        }
    }

    pub(super) fn key(&mut self, key: KeyEvent) {
        let count = self.fields.len();
        let Some(field) = self.fields.get_mut(self.selected) else {
            return;
        };
        match (key.code, &field.kind) {
            (KeyCode::Up, _) => self.selected = self.selected.saturating_sub(1),
            (KeyCode::Down, _) => self.selected = (self.selected + 1).min(count.saturating_sub(1)),
            (KeyCode::Left | KeyCode::Right, Kind::Boolean) => {
                field.value = String::from(if field.value == "true" {
                    "false"
                } else {
                    "true"
                });
            }
            (KeyCode::Left | KeyCode::Right, Kind::Choice(choices)) if !choices.is_empty() => {
                let index = choices.iter().position(|choice| *choice == field.value);
                let next = match (key.code, index) {
                    (KeyCode::Right, Some(index)) => (index + 1) % choices.len(),
                    (KeyCode::Left, Some(index)) => (index + choices.len() - 1) % choices.len(),
                    _ => 0,
                };
                field.value.clone_from(&choices[next]);
            }
            (KeyCode::Backspace, Kind::Text | Kind::Integer | Kind::Number | Kind::Array) => {
                field.value.pop();
            }
            // A control or alt chord is not text.
            (KeyCode::Char(character), Kind::Text | Kind::Integer | Kind::Number | Kind::Array)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                field.value.push(character);
            }
            _ => {}
        }
    }

    /// The accept content: `None` without a schema, or the form's values once
    /// they pass the same schema check the daemon applies.
    pub(super) fn content(&self) -> Result<Option<Value>, String> {
        let Some(schema) = &self.schema else {
            return Ok(None);
        };
        let mut content = Map::new();
        for field in &self.fields {
            if field.value.is_empty() {
                if field.required {
                    return Err(format!("{} is required", field.name));
                }
                continue;
            }
            let value = match field.kind {
                Kind::Array => serde_json::from_str::<Value>(&field.value)
                    .ok()
                    .filter(Value::is_array)
                    .ok_or_else(|| format!("{} must be a JSON array", field.name))?,
                Kind::Integer => field
                    .value
                    .parse::<i64>()
                    .map(Value::from)
                    .map_err(|_| format!("{} must be a whole number", field.name))?,
                Kind::Number => field
                    .value
                    .parse::<f64>()
                    .ok()
                    .and_then(serde_json::Number::from_f64)
                    .map(Value::Number)
                    .ok_or_else(|| format!("{} must be a number", field.name))?,
                Kind::Boolean => Value::Bool(field.value == "true"),
                Kind::Text | Kind::Choice(_) => Value::from(field.value.clone()),
            };
            content.insert(field.name.clone(), value);
        }
        let content = Value::Object(content);
        if crate::approvals::valid_response(schema, &content) {
            Ok(Some(content))
        } else {
            Err(String::from(
                "the answer does not match the question's schema",
            ))
        }
    }

    /// The card's lines wrapped to `width` columns that `rows` rows show:
    /// the title, the message and one line per field.
    pub(super) fn lines(&self, width: usize, rows: usize) -> Vec<Line<'static>> {
        let wrapped = self.wrapped(width);
        self.shown(&wrapped, rows)
            .into_iter()
            .map(|row| match row {
                Some(index) => wrapped[index].2.clone(),
                None => Line::from(Span::styled("…", Style::new().add_modifier(Modifier::DIM))),
            })
            .collect()
    }

    /// Every wrapped row: the card line it belongs to (0 is the title, then
    /// the message lines, then the fields), whether it continues that line,
    /// and its text.
    fn wrapped(&self, width: usize) -> Vec<(usize, bool, Line<'static>)> {
        self.rows()
            .iter()
            .enumerate()
            .flat_map(|(index, row)| {
                super::view::wrap(row, width)
                    .into_iter()
                    .enumerate()
                    .map(move |(part, line)| (index, part > 0, line))
            })
            .collect()
    }

    /// The wrapped rows that `rows` rows show, by index; `None` is a `…` row
    /// for skipped text. When they do not all fit, the title stays and the
    /// end of the card shows, where the question and the fields are, or the
    /// rows from the selected field when it is further up.
    fn shown(&self, wrapped: &[(usize, bool, Line<'static>)], rows: usize) -> Vec<Option<usize>> {
        let (total, rows) = (wrapped.len(), rows.max(1));
        if total <= rows {
            return (0..total).map(Some).collect();
        }
        let field = 1 + self.message.lines().count() + self.selected;
        let anchor = (!self.fields.is_empty())
            .then(|| wrapped.iter().position(|(line, _, _)| *line == field))
            .flatten();
        // Too few rows for marks: only the window.
        if rows < 4 {
            let start = anchor.map_or(total - rows, |anchor| anchor.min(total - rows));
            return (start..start + rows).map(Some).collect();
        }
        let tail = total - (rows - 2);
        let mut shown = vec![Some(0)];
        match anchor.filter(|anchor| *anchor < tail) {
            None => {
                shown.push(None);
                shown.extend((tail..total).map(Some));
            }
            // The selected field is above the end: from the top when it fits.
            Some(anchor) if anchor < rows - 1 => {
                shown.extend((1..rows - 1).map(Some));
                shown.push(None);
            }
            Some(anchor) => {
                shown.push(None);
                shown.extend((anchor..anchor + rows - 3).map(Some));
                shown.push(None);
            }
        }
        shown
    }

    /// The card's lines before wrapping, as styled segments.
    fn rows(&self) -> Vec<Vec<(String, Style)>> {
        let mut title = Style::new().add_modifier(Modifier::BOLD);
        if self.destructive {
            title = title.fg(Color::Red);
        }
        let mut rows = vec![vec![(
            self.title
                .clone()
                .unwrap_or_else(|| String::from("Question")),
            title,
        )]];
        rows.extend(
            self.message
                .lines()
                .map(|line| vec![(line.to_owned(), Style::new())]),
        );
        for (index, field) in self.fields.iter().enumerate() {
            let marker = if index == self.selected { "▸ " } else { "  " };
            let hint = match &field.kind {
                Kind::Choice(choices) => format!("  ({})", choices.join("/")),
                Kind::Boolean => String::from("  (true/false)"),
                Kind::Array => match self.schema.as_ref() {
                    Some(schema) => format!(
                        "  (JSON array; items: {})",
                        schema["properties"][&field.name]["items"]
                    ),
                    None => String::from("  (JSON array)"),
                },
                _ => String::new(),
            };
            let required = if field.required { "*" } else { "" };
            rows.push(vec![
                (format!("{marker}{}{required}: ", field.name), Style::new()),
                (field.value.clone(), Style::new().fg(Color::Cyan)),
                (hint, Style::new().add_modifier(Modifier::DIM)),
            ]);
        }
        rows
    }

    /// The button row drawn under the card, and each button's columns
    /// (start, end) and action.
    pub(super) fn buttons(&self) -> (Line<'static>, Vec<(u16, u16, &'static str)>) {
        let mut spans = Vec::new();
        let mut ranges = Vec::new();
        let mut column: u16 = 0;
        for (action, label) in BUTTONS {
            let width = u16::try_from(label.width()).unwrap_or(u16::MAX);
            ranges.push((column, column.saturating_add(width), action));
            column = column.saturating_add(width).saturating_add(1);
            spans.push(Span::styled(
                label,
                Style::new().add_modifier(Modifier::REVERSED),
            ));
            spans.push(Span::raw(" "));
        }
        let keys = if self.fields.is_empty() {
            " a · d · c · Esc back"
        } else {
            " Enter accept · ↑↓ field · ←→ choose · Esc back"
        };
        spans.push(Span::styled(keys, Style::new().add_modifier(Modifier::DIM)));
        (Line::from(spans), ranges)
    }

    /// The action of the button at `column` of the button row.
    pub(super) fn button_at(&self, column: u16) -> Option<&'static str> {
        self.buttons()
            .1
            .into_iter()
            .find(|(start, end, _)| (*start..*end).contains(&column))
            .map(|(_, _, action)| action)
    }

    /// A click on line `line` of the card drawn by `lines(width, rows)`, at
    /// `column`: a field's line selects it, and a click on a choice or yes/no
    /// value (past its name, or on a wrapped line) cycles it.
    pub(super) fn click(&mut self, line: usize, column: u16, width: usize, rows: usize) {
        let first = 1 + self.message.lines().count();
        let wrapped = self.wrapped(width);
        let Some(Some(row)) = self.shown(&wrapped, rows).get(line).copied() else {
            return;
        };
        let (index, wrapped) = (wrapped[row].0, wrapped[row].1);
        let Some(index) = index
            .checked_sub(first)
            .filter(|index| *index < self.fields.len())
        else {
            return;
        };
        self.selected = index;
        let field = &self.fields[index];
        let required = if field.required { "*" } else { "" };
        let value_start = format!("▸ {}{required}: ", field.name).width();
        if wrapped || usize::from(column) >= value_start {
            self.key(KeyEvent::from(KeyCode::Right));
        }
    }
}

/// Form fields in schema order, required fields first.
fn fields(schema: &Value) -> Vec<Field> {
    let required: Vec<&str> = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut fields: Vec<Field> = schema["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, property)| {
            let kind = match property["type"].as_str() {
                _ if property["enum"].is_array() => Kind::Choice(
                    property["enum"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|choice| {
                            choice
                                .as_str()
                                .map_or_else(|| choice.to_string(), str::to_owned)
                        })
                        .collect(),
                ),
                Some("boolean") => Kind::Boolean,
                Some("integer") => Kind::Integer,
                Some("number") => Kind::Number,
                Some("array") => Kind::Array,
                _ => Kind::Text,
            };
            Field {
                name: name.clone(),
                kind,
                required: required.contains(&name.as_str()),
                value: String::new(),
            }
        })
        .collect();
    fields.sort_by_key(|field| !field.required);
    fields
}

#[cfg(test)]
mod tests;
