//! Markdown events rendered as wrapped terminal lines.
use super::{
    CodeBlockKind, Color, Event, HeadingLevel, Line, Modifier, Options, Parser, Span, Style, Tag,
    TagEnd, display_width, wrap_hanging,
};

/// Markdown as styled, wrapped lines.
pub(in crate::cli::attach) fn markdown(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut writer = Markdown {
        width,
        out: Vec::new(),
        line: Vec::new(),
        styles: vec![Style::new()],
        lists: Vec::new(),
        items: Vec::new(),
        marked: false,
        quote: 0,
        code: None,
    };
    for event in Parser::new_ext(text, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH) {
        writer.event(event);
    }
    writer.flush();
    // An unterminated fence while streaming still shows its text.
    if let Some(block) = writer.code.take() {
        writer.code_block(&block);
    }
    writer.out
}

struct Markdown {
    width: usize,
    out: Vec<Line<'static>>,
    line: Vec<(String, Style)>,
    styles: Vec<Style>,
    /// Open lists, with the next number of an ordered one.
    lists: Vec<Option<u64>>,
    /// Open list items: the width of each one's marker, so wrapped lines and
    /// later paragraphs line up under its text.
    items: Vec<usize>,
    /// The pending line starts with its item's marker.
    marked: bool,
    quote: usize,
    code: Option<String>,
}

impl Markdown {
    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }
    /// The innermost open item's marker width.
    fn hang(&self) -> usize {
        self.items.last().copied().unwrap_or(0)
    }
    fn flush(&mut self) {
        if self.line.is_empty() {
            return;
        }
        let hang = self.hang();
        // A later paragraph of an item lines up under the item's text.
        if hang > 0 && !self.marked {
            self.line.insert(0, (" ".repeat(hang), Style::new()));
        }
        let mut bar = 0;
        if self.quote > 0 {
            let prefix = "│ ".repeat(self.quote);
            bar = display_width(&prefix);
            self.line
                .insert(0, (prefix, Style::new().add_modifier(Modifier::DIM)));
        }
        self.out
            .extend(wrap_hanging(&self.line, self.width, bar + hang));
        self.line.clear();
        self.marked = false;
    }
    fn code_block(&mut self, block: &str) {
        let style = Style::new().fg(Color::Green);
        let indent = " ".repeat(self.hang() + 2);
        for code_line in block.lines() {
            self.out.extend(wrap_hanging(
                &[(format!("{indent}{code_line}"), style)],
                self.width,
                indent.len(),
            ));
        }
    }
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => match &mut self.code {
                Some(block) => block.push_str(&text),
                None => self.line.push((text.into_string(), self.style())),
            },
            Event::Code(text) => {
                self.line
                    .push((text.into_string(), Style::new().fg(Color::Cyan)));
            }
            Event::SoftBreak => self.line.push((String::from(" "), self.style())),
            Event::HardBreak => self.flush(),
            Event::Rule => {
                self.flush();
                self.out.push(Line::from("─".repeat(self.width.min(40))));
            }
            Event::Html(text) | Event::InlineHtml(text) => {
                self.line.push((text.into_string(), self.style()));
            }
            Event::TaskListMarker(done) => {
                let marker = if done { "[x] " } else { "[ ] " };
                self.line.push((String::from(marker), Style::new()));
            }
            _ => {}
        }
    }
    fn start(&mut self, tag: Tag<'_>) {
        let style = self.style();
        match tag {
            Tag::Heading { level, .. } => {
                self.flush();
                let bold = Style::new().add_modifier(Modifier::BOLD);
                self.styles.push(if level == HeadingLevel::H1 {
                    bold.add_modifier(Modifier::UNDERLINED)
                } else {
                    bold
                });
            }
            Tag::Emphasis => self.styles.push(style.add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.styles.push(style.add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self.styles.push(style.add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { .. } => self.styles.push(style.fg(Color::Blue)),
            Tag::BlockQuote(_) => {
                self.flush();
                self.quote += 1;
            }
            Tag::List(start) => {
                self.flush();
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush();
                let indent = "  ".repeat(self.lists.len().saturating_sub(1));
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        let marker = format!("{indent}{number}. ");
                        *number += 1;
                        marker
                    }
                    _ => format!("{indent}• "),
                };
                self.items.push(display_width(&marker));
                self.marked = true;
                self.line.push((marker, Style::new()));
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                if let CodeBlockKind::Fenced(language) = &kind
                    && !language.is_empty()
                {
                    let dim = Style::new().add_modifier(Modifier::DIM);
                    self.out
                        .push(Line::from(Span::styled(format!("  {language}"), dim)));
                }
                self.code = Some(String::new());
            }
            _ => {}
        }
    }
    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                self.styles.pop();
                self.flush();
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                self.styles.pop();
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.quote = self.quote.saturating_sub(1);
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
            }
            TagEnd::Item => {
                self.flush();
                self.items.pop();
            }
            TagEnd::Paragraph | TagEnd::TableRow | TagEnd::TableHead => self.flush(),
            TagEnd::CodeBlock => {
                let block = self.code.take().unwrap_or_default();
                self.code_block(&block);
            }
            TagEnd::TableCell => self.line.push((String::from(" │ "), Style::new())),
            _ => {}
        }
    }
}
