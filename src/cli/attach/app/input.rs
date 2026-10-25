//! Input for the App view.
use super::{
    ARGUMENTS, App, Command, DOUBLE_ESC, Event, Exit, Focus, Instant, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers, Model, MouseButton, MouseEvent, MouseEventKind, Position, Rect,
    Shown, common_start, matching,
};

use super::requests;

impl App {
    /// Handle one terminal event; `false` when it changed nothing, so the
    /// frame need not be redrawn. Only mouse events can change nothing.
    pub(in crate::cli::attach) fn event(&mut self, event: &Event) -> bool {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.key(*key),
            Event::Paste(text) if !self.mode.read_only && self.focus == Focus::Input => {
                self.completing = None;
                self.insert(text);
            }
            Event::Paste(text)
                if !self.mode.read_only && self.focus == Focus::Card && self.picker.is_none() =>
            {
                if let Some(card) = &mut self.card {
                    card.paste(text);
                }
            }
            Event::Mouse(mouse) => return self.mouse(*mouse),
            _ => {}
        }
        true
    }

    /// A left click focuses the part of the view under it; the wheel scrolls
    /// the transcript three lines a step. Other mouse events change nothing.
    pub(super) fn mouse(&mut self, event: MouseEvent) -> bool {
        let point = Position::new(event.column, event.row);
        let regions = self.shown.regions;
        if !regions.area.contains(point) {
            return false;
        }
        match event.kind {
            MouseEventKind::ScrollUp => self.scroll_up(3),
            MouseEventKind::ScrollDown => self.scroll_down(3),
            MouseEventKind::Down(MouseButton::Left) => {
                self.quit_armed = false;
                if regions.input.contains(point) {
                    self.focus = Focus::Input;
                } else if regions.card.contains(point) && self.has_card() {
                    self.focus = Focus::Card;
                    self.card_click(regions.card, point);
                } else if regions.transcript.contains(point) {
                    self.focus = Focus::Transcript;
                }
            }
            _ => return false,
        }
        true
    }

    pub(super) fn key(&mut self, key: KeyEvent) {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if control && key.code == KeyCode::Char('c') {
            self.quit();
            return;
        }
        self.quit_armed = false;
        if key.code != KeyCode::Tab {
            self.completing = None;
        }
        match key.code {
            KeyCode::PageUp => return self.scroll_up(self.shown.height.max(1)),
            KeyCode::PageDown => return self.scroll_down(self.shown.height.max(1)),
            // In the input, Tab finishes a view command; otherwise it moves focus.
            KeyCode::Tab if self.focus == Focus::Input && self.complete() => return,
            KeyCode::Tab => return self.cycle_focus(),
            _ => {}
        }
        match self.focus {
            Focus::Transcript => match key.code {
                KeyCode::Up => self.scroll_up(1),
                KeyCode::Down => self.scroll_down(1),
                KeyCode::Esc => self.focus = Focus::Input,
                _ => {}
            },
            Focus::Card => self.card_key(key),
            Focus::Input if !self.mode.read_only => self.input_key(key),
            Focus::Input => {}
        }
    }

    pub(super) fn quit(&mut self) {
        if self.in_flight.is_empty() || self.quit_armed {
            self.exit = Some(Exit::Quit);
        } else {
            self.quit_armed = true;
            self.status = String::from("a write is still in flight; Ctrl-C again to quit anyway");
        }
    }

    /// The partly typed view-command name: `/` and one word, not `//`, with
    /// the cursor at its end.
    pub(super) fn slash_word(&self) -> Option<&str> {
        let word = self.input.strip_prefix('/')?;
        (!self.mode.read_only
            && !word.starts_with('/')
            && !word.contains(char::is_whitespace)
            && self.cursor == self.input.chars().count())
        .then_some(word)
    }

    /// Help while a view command is typed: its arguments once the name is
    /// complete, otherwise the commands Tab completes to.
    pub(super) fn slash_hint(&self) -> Option<String> {
        let command = self.input.strip_prefix('/')?;
        if let Some((name, rest)) = command.split_once(' ')
            && rest.trim().is_empty()
        {
            let (_, usage) = ARGUMENTS.iter().find(|(known, _)| *known == name)?;
            return (!self.mode.read_only).then(|| format!("usage: {usage}"));
        }
        let word = self.slash_word()?;
        if let Some((_, usage)) = ARGUMENTS.iter().find(|(known, _)| *known == word) {
            return Some(format!("usage: {usage}"));
        }
        let prefix = self
            .completing
            .as_ref()
            .map_or(word, |(prefix, _)| prefix.as_str());
        let names: Vec<String> = matching(prefix)
            .into_iter()
            .map(|name| format!("/{name}"))
            .collect();
        Some(if names.is_empty() {
            String::from("no view command starts so; Enter sends it to the agent")
        } else {
            format!("Tab completes: {}", names.join(" "))
        })
    }

    /// Complete a partly typed view command, true when the input is one. A
    /// single match is filled in, with a space when it takes arguments;
    /// several extend to their common start, then Tab cycles through them.
    pub(super) fn complete(&mut self) -> bool {
        let Some(word) = self.slash_word().map(str::to_owned) else {
            return false;
        };
        let prefix = self
            .completing
            .as_ref()
            .map_or_else(|| word.clone(), |(prefix, _)| prefix.clone());
        let names = matching(&prefix);
        let text = match names.as_slice() {
            [] => return true,
            [only] => {
                self.completing = None;
                let takes_arguments = ARGUMENTS
                    .iter()
                    .any(|(name, usage)| name == only && usage.contains(' '));
                format!("/{only}{}", if takes_arguments { " " } else { "" })
            }
            several => {
                let common = common_start(several);
                if self.completing.is_none() && common.len() > word.len() {
                    format!("/{common}")
                } else {
                    let next = self
                        .completing
                        .as_ref()
                        .map_or(0, |(_, index)| (index + 1) % several.len());
                    self.completing = Some((prefix, next));
                    format!("/{}", several[next])
                }
            }
        };
        self.cursor = text.chars().count();
        self.input = text;
        true
    }

    pub(super) fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Input if self.has_card() => Focus::Card,
            Focus::Input | Focus::Card => Focus::Transcript,
            Focus::Transcript if self.mode.read_only && self.has_card() => Focus::Card,
            Focus::Transcript => Focus::Input,
        };
    }

    pub(super) fn scroll_up(&mut self, lines: usize) {
        let Shown {
            lines: rendered,
            height,
            more,
            ..
        } = self.shown;
        // Unrendered older entries mean there is more to scroll; the next frame
        // renders enough of them.
        if more {
            self.scroll += lines;
            return;
        }
        let top = rendered.saturating_sub(height);
        self.scroll = (self.scroll + lines).min(top);
        if self.scroll >= top
            && !self.history_pending
            && let Some(before) = self.model.as_ref().and_then(Model::older)
        {
            self.history_pending = true;
            self.commands.push(Command::History { before });
        }
    }

    pub(super) fn scroll_down(&mut self, lines: usize) {
        self.clamp_scroll();
        self.scroll = self.scroll.saturating_sub(lines);
    }

    /// Once the whole transcript is rendered, never stay scrolled past its top.
    pub(in crate::cli::attach) fn clamp_scroll(&mut self) {
        if !self.shown.more {
            self.scroll = self
                .scroll
                .min(self.shown.lines.saturating_sub(self.shown.height));
        }
    }

    /// A click inside the card: a button answers as its key does; a field
    /// line selects the field or cycles its value.
    pub(super) fn card_click(&mut self, area: Rect, point: Position) {
        // Inside the border; the button row is the last inner row.
        let inner = Rect::new(
            area.x + 1,
            area.y + 1,
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        );
        if !inner.contains(point) {
            return;
        }
        // The footer is always drawn on the last inner row, even when a short
        // view squeezes the card over its lines.
        let footer = point.y + 1 == inner.y + inner.height;
        // The same row budget as the frame, so scrolled lines line up.
        let rows = super::super::view::card_rows(self.shown.regions.area.height);
        let line = usize::from(point.y - inner.y);
        if let Some(picker) = &mut self.picker {
            if let Some(index) = picker.at(line, rows).filter(|_| !footer) {
                picker.select(index);
                self.pick();
            }
            return;
        }
        let Some(card) = &mut self.card else {
            return;
        };
        if footer {
            if let Some(action) = card.button_at(point.x - inner.x) {
                self.respond(action);
            }
        } else {
            card.click(line, point.x - inner.x, usize::from(inner.width), rows);
        }
    }

    pub(super) fn input_key(&mut self, key: KeyEvent) {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code != KeyCode::Esc {
            self.last_esc = None;
        }
        match key.code {
            KeyCode::Enter if alt => self.submit(Some("next-turn")),
            KeyCode::Enter => self.submit(None),
            KeyCode::Char('r') if control => self.submit(Some("redirect")),
            KeyCode::Char('j') if control => self.insert("\n"),
            // In the input, Esc closes an open picker, never counting towards
            // a double-Esc interrupt.
            KeyCode::Esc if self.picker.is_some() => {
                self.last_esc = None;
                self.close_picker();
            }
            KeyCode::Esc => self.escape(),
            KeyCode::Char(character) if !control => {
                let mut buffer = [0; 4];
                self.insert(character.encode_utf8(&mut buffer));
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.remove(self.cursor);
            }
            KeyCode::Delete => self.remove(self.cursor),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.input.chars().count()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.chars().count(),
            KeyCode::Up => self.scroll_up(1),
            KeyCode::Down => self.scroll_down(1),
            _ => {}
        }
    }

    pub(super) fn escape(&mut self) {
        let now = Instant::now();
        if self
            .last_esc
            .is_some_and(|last| now.duration_since(last) <= DOUBLE_ESC)
        {
            self.last_esc = None;
            self.act("interrupt", requests::control("interrupt", &self.target));
        } else {
            self.last_esc = Some(now);
        }
    }

    pub(super) fn insert(&mut self, text: &str) {
        let index = self.byte_index(self.cursor);
        self.input.insert_str(index, text);
        self.cursor += text.chars().count();
    }

    pub(super) fn remove(&mut self, position: usize) {
        if position < self.input.chars().count() {
            let index = self.byte_index(position);
            self.input.remove(index);
        }
    }

    pub(super) fn byte_index(&self, position: usize) -> usize {
        self.input
            .char_indices()
            .nth(position)
            .map_or(self.input.len(), |(index, _)| index)
    }

    pub(super) fn card_key(&mut self, key: KeyEvent) {
        if let Some(picker) = &mut self.picker {
            // The TUI's prefix and other chords never pick.
            if key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            {
                return;
            }
            match key.code {
                KeyCode::Up => picker.up(),
                KeyCode::Down => picker.down(),
                KeyCode::Enter => self.pick(),
                KeyCode::Esc => self.close_picker(),
                _ => {}
            }
            return;
        }
        let Some(card) = &mut self.card else {
            self.focus = Focus::Input;
            return;
        };
        // Only plain letters answer: Ctrl-A (the TUI's prefix sent twice) or
        // Ctrl-D must never accept or decline.
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => self.focus = Focus::Input,
            // Single keys answer only a simple approval; a form needs Enter or a
            // slash command, so a stray letter cannot decline it.
            KeyCode::Char('a') if plain && card.simple() => self.respond("accept"),
            KeyCode::Char('d') if plain && card.simple() => self.respond("decline"),
            KeyCode::Char('c') if plain && card.simple() => self.respond("cancel"),
            KeyCode::Enter => self.respond("accept"),
            _ => card.key(key),
        }
    }
}
