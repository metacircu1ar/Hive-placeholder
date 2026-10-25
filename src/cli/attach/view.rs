//! Render the model: header, wrapped transcript, approval card, input and keys.
mod markdown;

pub(super) use markdown::markdown;

use super::model::{Entry, Header, Model};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use unicode_width::UnicodeWidthChar;

/// Output lines shown under a collapsed tool call.
const TOOL_TAIL_LINES: usize = 3;

/// Everything one frame shows; the app owns the state.
pub(super) struct Screen<'a> {
    pub model: &'a Model,
    /// Transcript lines scrolled up from the bottom.
    pub scroll: usize,
    pub input: Option<(&'a str, usize)>,
    pub focus: Focus,
    /// The question card, or the picker shown in its place.
    pub card: Option<Panel>,
    pub status: &'a str,
    /// Help for the view command being typed, shown on the input's border so
    /// the status line keeps its messages.
    pub completion: Option<String>,
    pub connected: bool,
    /// Only the active view places the terminal cursor.
    pub active: bool,
}

/// What the card's place shows: a frame title, lines and the row under them.
#[derive(Debug)]
pub(super) struct Panel {
    pub title: String,
    pub lines: Vec<Line<'static>>,
    pub footer: Line<'static>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Focus {
    #[default]
    Input,
    Card,
    Transcript,
}

/// What one frame showed of the transcript, for scrolling, and where each
/// part was drawn, for the mouse.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Shown {
    /// Wrapped lines rendered, from the newest up.
    pub lines: usize,
    pub height: usize,
    /// Older entries exist above the rendered lines.
    pub more: bool,
    pub regions: Regions,
}

/// The areas one frame drew; empty when a part was not shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Regions {
    pub area: Rect,
    pub transcript: Rect,
    pub card: Rect,
    pub input: Rect,
}

/// Draw the view into `area` and report how much of the transcript it rendered.
pub(super) fn draw(frame: &mut Frame<'_>, area: Rect, screen: &Screen<'_>) -> Shown {
    // The card's lines, then the button row, inside the border.
    let most = card_rows(area.height);
    let card_height = screen.card.as_ref().map_or(0, |panel| {
        u16::try_from(panel.lines.len().min(most) + 3).unwrap_or(u16::MAX)
    });
    let input_height = if screen.input.is_some() { 3 } else { 0 };
    let [header, body, card, input, keys] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(card_height),
        Constraint::Length(input_height),
        Constraint::Length(1),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(header_lines(&screen.model.header, screen)),
        header,
    );
    let height = usize::from(body.height);
    // Only the newest lines the window needs are wrapped, however long the history.
    let (lines, more) = transcript(
        screen.model,
        usize::from(body.width.max(1)),
        screen.scroll + height,
    );
    let end = lines
        .len()
        .saturating_sub(screen.scroll.min(lines.len().saturating_sub(height)));
    let start = end.saturating_sub(height);
    frame.render_widget(Paragraph::new(lines[start..end].to_vec()), body);
    if let Some(panel) = &screen.card {
        draw_card(frame, card, panel, screen.focus == Focus::Card);
    }
    if let Some((text, cursor)) = screen.input {
        let focused = screen.focus == Focus::Input;
        draw_input(
            frame,
            input,
            (text, cursor),
            screen.completion.as_deref(),
            (focused, focused && screen.active),
        );
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            screen.status.to_owned(),
            Style::new().add_modifier(Modifier::DIM),
        ))),
        keys,
    );
    Shown {
        lines: lines.len(),
        height,
        more,
        regions: Regions {
            area,
            transcript: body,
            card: if screen.card.is_some() {
                card
            } else {
                Rect::default()
            },
            input: if screen.input.is_some() {
                input
            } else {
                Rect::default()
            },
        },
    }
}

/// The most text rows a card shows in a view `height` rows tall: half the
/// view, and at least nine.
pub(super) fn card_rows(height: u16) -> usize {
    usize::from(height / 2).max(9)
}

/// The question card or picker: its lines, then the footer row on its last
/// inner row, however long the question is.
fn draw_card(frame: &mut Frame<'_>, area: Rect, panel: &Panel, focused: bool) {
    let style = if focused {
        Style::new().fg(Color::Yellow)
    } else {
        Style::new().add_modifier(Modifier::DIM)
    };
    frame.render_widget(Clear, area);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(style)
        .title(format!(" {} ", panel.title));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [text, row] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    frame.render_widget(Paragraph::new(panel.lines.clone()), text);
    frame.render_widget(Paragraph::new(panel.footer.clone()), row);
}

fn header_lines(header: &Header, screen: &Screen<'_>) -> Vec<Line<'static>> {
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let dim = Style::new().add_modifier(Modifier::DIM);
    // The transcript's trailing "…" shows streaming; the header keeps the state.
    let state = if header.ended {
        format!("ended ({})", header.outcome)
    } else {
        header.state.clone()
    };
    // A name leads; the ID stays, dimmed, since names are not unique and
    // commands take IDs.
    let mut first = match &header.name {
        Some(name) => vec![
            Span::styled(name.clone(), bold),
            Span::styled(format!(" {}", header.id), dim),
        ],
        None => vec![Span::styled(header.id.clone(), bold)],
    };
    if header.sandbox == "full-access" {
        first.insert(0, Span::styled("[full-access] ", bold.fg(Color::Red)));
    }
    first.extend([
        Span::raw(
            header
                .role
                .as_ref()
                .map_or_else(String::new, |role| format!(" {role}")),
        ),
        Span::raw(format!("  {state}")),
    ]);
    if let Some(waiting) = &header.waiting {
        first.push(Span::styled(
            format!(" (waiting on {waiting})"),
            Style::new().fg(Color::Yellow),
        ));
    }
    if !screen.connected {
        first.push(Span::styled(
            "  disconnected, retrying",
            Style::new().fg(Color::Red),
        ));
    }
    let decision = header
        .auth_decision
        .as_ref()
        .map_or_else(String::new, |reason| format!(" ({reason})"));
    let parent = header
        .parent
        .as_ref()
        .map_or_else(String::new, |parent| format!("  parent {parent}"));
    let second = format!(
        "{} {}  login {}{decision}{parent}  children {}  queued {}",
        header.model, header.effort, header.auth, header.children, header.queued
    );
    vec![Line::from(first), Line::from(Span::styled(second, dim))]
}

fn draw_input(
    frame: &mut Frame<'_>,
    area: Rect,
    (text, cursor): (&str, usize),
    completion: Option<&str>,
    (focused, place_cursor): (bool, bool),
) {
    let style = if focused {
        Style::new()
    } else {
        Style::new().add_modifier(Modifier::DIM)
    };
    let mut block = Block::new().borders(Borders::TOP).border_style(style);
    if let Some(completion) = completion {
        block = block.title(Span::styled(
            format!(" {completion} "),
            Style::new().add_modifier(Modifier::DIM),
        ));
    }
    let inner = block.inner(area);
    let width = usize::from(inner.width.max(1));
    // Show the line holding the cursor, scrolled horizontally to keep it visible.
    let before: String = text.chars().take(cursor).collect();
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    let line_end = text[line_start..]
        .find('\n')
        .map_or(text.len(), |index| line_start + index);
    let column = display_width(&before[line_start..]);
    let shift = column.saturating_sub(width.saturating_sub(1));
    let visible: String = skip_columns(&text[line_start..line_end], shift);
    frame.render_widget(
        Paragraph::new(Line::from(visible))
            .style(style)
            .block(block),
        area,
    );
    if place_cursor {
        let x = inner.x + u16::try_from(column - shift).unwrap_or(inner.width);
        frame.set_cursor_position((x.min(inner.x + inner.width.saturating_sub(1)), inner.y));
    }
}

/// The newest transcript lines, at least `needed` of them when that many exist,
/// oldest first; `true` when older entries were left unrendered.
pub(super) fn transcript(model: &Model, width: usize, needed: usize) -> (Vec<Line<'static>>, bool) {
    let dim = Style::new().add_modifier(Modifier::DIM);
    let mut blocks = Vec::new();
    let mut count = 0;
    if model.streaming {
        blocks.push(vec![Line::from(Span::styled("…", dim))]);
        count += 1;
    }
    let mut entries = model.entries().rev();
    let mut more = false;
    for entry in entries.by_ref() {
        let mut lines = Vec::new();
        entry_lines(entry, width, &mut lines);
        count += lines.len();
        blocks.push(lines);
        if count >= needed {
            break;
        }
    }
    if entries.next().is_some() {
        more = true;
    } else {
        let notice = if model.history_expired() {
            Some("(older history was compacted)")
        } else if model.full() {
            Some("(older entries are not kept in memory; hivectl read shows the full history)")
        } else if model.older().is_some() {
            Some("(PageUp for older history)")
        } else {
            None
        };
        if let Some(notice) = notice {
            blocks.push(vec![Line::from(Span::styled(notice, dim))]);
        }
    }
    (blocks.into_iter().rev().flatten().collect(), more)
}

fn entry_lines(entry: &Entry, width: usize, lines: &mut Vec<Line<'static>>) {
    let dim = Style::new().add_modifier(Modifier::DIM);
    match entry {
        Entry::Input { sender, body, mode } => {
            let label = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
            let mode = mode
                .as_deref()
                .filter(|mode| *mode != "next-turn")
                .map_or_else(String::new, |mode| format!(" [{mode}]"));
            let mut segments = vec![(format!("{sender}{mode}: "), label)];
            segments.push((body.clone(), Style::new()));
            lines.extend(wrap(&segments, width));
        }
        Entry::Assistant { text, live, .. } => {
            let shown = if live.len() > text.len() { live } else { text };
            lines.extend(markdown(shown, width));
        }
        Entry::Reasoning { text, .. } => {
            let style = dim.add_modifier(Modifier::ITALIC);
            lines.extend(wrap(&[(format!("· {text}"), style)], width));
        }
        Entry::Tool {
            tool,
            summary,
            status,
            output,
            running,
            ..
        } => {
            let state = if *running {
                String::from("running")
            } else {
                status.clone().unwrap_or_else(|| String::from("done"))
            };
            let colour = if *running {
                Color::Yellow
            } else {
                Color::Green
            };
            let segments = [
                (format!("⚙ {tool} "), Style::new().fg(colour)),
                (summary.clone(), Style::new()),
                (format!("  {state}"), dim),
            ];
            lines.extend(wrap(&segments, width));
            let tail: Vec<&str> = output.lines().rev().take(TOOL_TAIL_LINES).collect();
            for line in tail.into_iter().rev() {
                lines.extend(wrap(&[(format!("  {line}"), dim)], width));
            }
        }
        Entry::TurnStarted { model, effort } => {
            lines.push(Line::from(Span::styled(
                format!("── turn: {model} {effort}"),
                dim,
            )));
        }
        Entry::TurnFinished { status } => {
            lines.push(Line::from(Span::styled(format!("── {status}"), dim)));
        }
        Entry::Approval { id, title, open } => {
            let state = if *open { "waiting" } else { "closed" };
            let title = title.clone().unwrap_or_else(|| id.clone());
            lines.extend(wrap(
                &[(
                    format!("? {title} ({state})"),
                    Style::new().fg(Color::Yellow),
                )],
                width,
            ));
        }
        Entry::Notice(text) => lines.extend(wrap(&[(format!("• {text}"), dim)], width)),
    }
}

/// Greedy word wrap of styled segments to `width` columns.
pub(super) fn wrap(segments: &[(String, Style)], width: usize) -> Vec<Line<'static>> {
    wrap_hanging(segments, width, 0)
}

/// Word wrap whose continuation lines start with `hang` spaces, at most half
/// the width so a narrow view still has room for text.
pub(super) fn wrap_hanging(
    segments: &[(String, Style)],
    width: usize,
    hang: usize,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let hang = hang.min(width / 2);
    let mut lines = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut column = 0;
    // End the current line and start a continuation line.
    let mut next_line = |current: &mut Vec<Span<'static>>, column: &mut usize| {
        lines.push(Line::from(std::mem::take(current)));
        *column = 0;
        if hang > 0 {
            current.push(Span::raw(" ".repeat(hang)));
            *column = hang;
        }
    };
    for (text, style) in segments {
        for (index, part) in text.split('\n').enumerate() {
            if index > 0 {
                next_line(&mut current, &mut column);
            }
            for word in split_keeping_spaces(part) {
                let word_width = display_width(word);
                if column + word_width > width && column > hang {
                    next_line(&mut current, &mut column);
                    if word.trim().is_empty() {
                        continue;
                    }
                }
                if column + word_width <= width {
                    current.push(Span::styled(word.to_owned(), *style));
                    column += word_width;
                    continue;
                }
                // A word longer than the room left breaks at the column limit.
                let mut piece = String::new();
                for character in word.chars() {
                    let character_width = character.width().unwrap_or(0);
                    if column + character_width > width {
                        current.push(Span::styled(std::mem::take(&mut piece), *style));
                        next_line(&mut current, &mut column);
                    }
                    piece.push(character);
                    column += character_width;
                }
                current.push(Span::styled(piece, *style));
            }
        }
    }
    lines.push(Line::from(current));
    lines
}

fn split_keeping_spaces(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let first_space = rest.starts_with(' ');
        let end = rest
            .char_indices()
            .find(|(_, character)| (*character == ' ') != first_space)
            .map_or(rest.len(), |(index, _)| index);
        let (word, remaining) = rest.split_at(end);
        rest = remaining;
        Some(word)
    })
}

pub(super) fn display_width(text: &str) -> usize {
    text.chars()
        .map(|character| character.width().unwrap_or(0))
        .sum()
}

fn skip_columns(text: &str, columns: usize) -> String {
    let mut skipped = 0;
    text.chars()
        .skip_while(|character| {
            let skip = skipped < columns;
            skipped += character.width().unwrap_or(0);
            skip
        })
        .collect()
}

#[cfg(test)]
mod tests;
