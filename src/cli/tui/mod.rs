//! `hivectl tui`: one terminal for many agents. A sidebar shows the agent tree;
//! tabs and splits hold `hivectl attach` views; an attention queue collects
//! pending questions; a dialog spawns agents.
mod interaction;
mod render;
mod updates;

mod attention;
mod keys;
mod layout;
mod pointer;
mod sidebar;
mod spawn;

use super::attach::{END_LINGER, Exit, Lists, Mode, View};
use super::tree_feed::{self, Feed};
use super::{Cli, Error, Result, api};
use crate::runtime;
use attention::Attention;
use keys::{Action, Prefix, Routed, Router};
use layout::{Layout, Opened, Place, Room};
use ratatui::{
    Frame,
    crossterm::{
        event::{
            self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent,
            KeyEventKind, KeyModifiers,
        },
        execute,
    },
    layout::{Constraint, Layout as Areas, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use sidebar::Sidebar;
use std::{
    collections::HashMap,
    io::IsTerminal,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{Receiver, Sender},
    },
    time::{Duration, Instant},
};

// The loop polls keys this often, so signals and updates are never late.
const TICK: Duration = Duration::from_millis(100);
/// A closed view's long-poll can stay in flight this long; it still counts
/// against `--max-views` meanwhile.
const CLOSED_POLL: Duration = Duration::from_secs(26);

#[derive(clap::Args)]
pub(super) struct Tui {
    /// Only this agent and its descendants; by default every live agent.
    #[arg(long, value_parser = hive_input::clap::parsers::text(crate::limits::CONTROL_INPUT_BYTES))]
    root: Option<String>,
    /// Most agent views open at once, 1–12 (default 6); each holds one daemon long-poll.
    #[arg(long = "max-views",
         default_value_t = 6,
         value_parser = clap::value_parser!(u16).range(1..=12))]
    limit: u16,
    /// Keep ended agents in the sidebar and their views open.
    #[arg(long)]
    keep_ended: bool,
    /// No terminal bell or notification when an agent needs you.
    #[arg(long)]
    quiet: bool,
    /// Leave the mouse to the terminal: no clicks or wheel scrolling in the TUI.
    #[arg(long)]
    no_mouse: bool,
}

pub(super) fn run(cli: &Cli, args: &Tui) -> i32 {
    let result = prepare(cli)
        .and_then(|(root, settings)| super::logging::run_held(|| ui(root, settings, args)));
    match result {
        Ok(None) => 0,
        Ok(Some(reply)) => {
            let code = super::views::error_code(&reply).as_str();
            let human = format!("Hive: cannot follow the agent tree ({code}).\n");
            super::parsing::emit_failure_record(&reply, human)
        }
        Err(error) => {
            let human = super::parsing::human(&error);
            super::parsing::emit_failure_record(&api::value(api::ErrorResponse { error }), human)
        }
    }
}

/// Check the terminal and read `tui.toml` before the terminal enters raw mode.
fn prepare(cli: &Cli) -> Result<(PathBuf, keys::Settings)> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(Error::interactive_terminal_required(
            "hivectl tui needs a terminal for stdin and stdout; use hivectl live for scripts.",
        ));
    }
    let config = cli.config_path()?;
    let settings = keys::load(config.parent().unwrap_or(Path::new("")))?;
    Ok((cli.state_root()?, settings))
}

fn ui(root: PathBuf, settings: keys::Settings, args: &Tui) -> Result<Option<Value>> {
    let signals = Arc::new(AtomicUsize::new(0));
    crate::os::install_cancellation(&signals).map_err(Error::state_io_from)?;
    let (sender, feed) = std::sync::mpsc::sync_channel(4);
    let (scope, follower) = (args.root.clone(), root.clone());
    std::thread::spawn(move || tree_feed::follow(follower, scope, sender));
    let (asks, pending) = std::sync::mpsc::channel();
    let (answered, answers) = std::sync::mpsc::channel();
    let worker = root.clone();
    std::thread::spawn(move || work(&worker, &pending, &answered));
    let mut terminal = ratatui::try_init().map_err(super::attach::terminal_error)?;
    super::hangup::watch();
    super::mouse::restore_on_panic();
    let pasting = execute!(std::io::stdout(), EnableBracketedPaste).is_ok();
    let clicking = settings.mouse
        && !args.no_mouse
        && execute!(std::io::stdout(), super::mouse::Reporting(true)).is_ok();
    let mut console = Console::new(root, settings.prefix, args, asks, answers);
    console.lists = Arc::new(settings.lists);
    let result = console.run(&mut terminal, &feed, &signals);
    if clicking {
        // Restoring the terminal below matters more than this mode.
        let _ = execute!(std::io::stdout(), super::mouse::Reporting(false));
    }
    if pasting {
        // Restoring the terminal below matters more than this mode.
        let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    }
    ratatui::try_restore().map_err(super::attach::terminal_error)?;
    result
}

/// A daemon request from the dialog, answered on the worker thread.
enum Ask {
    Roles,
    /// Sent once: `id` is fresh for every submission.
    Spawn {
        id: String,
        request: Value,
    },
}

enum Answer {
    Roles(std::result::Result<Value, String>),
    /// The reply to the spawn sent with `id`, or the daemon's error object.
    Spawned {
        id: String,
        result: std::result::Result<Value, Value>,
    },
}

fn work(root: &Path, pending: &Receiver<Ask>, answered: &Sender<Answer>) {
    while let Ok(ask) = pending.recv() {
        let answer = match ask {
            Ask::Roles => Answer::Roles(
                match runtime::client(
                    root,
                    &uuid::Uuid::new_v4().to_string(),
                    api::value(crate::runtime::api::Request::Roles),
                ) {
                    Ok(reply) => match reply.get("error") {
                        Some(_) => Err(super::views::error_code(&reply).as_str().to_owned()),
                        None => Ok(reply),
                    },
                    Err(_) => Err(String::from("the daemon did not answer")),
                },
            ),
            Ask::Spawn { id, request } => {
                let result = match runtime::client(root, &id, &request) {
                    Ok(reply) => match reply.get("error") {
                        Some(error) => Err(error.clone()),
                        None => Ok(reply),
                    },
                    Err(_) => Err(api::value(Error::daemon_unavailable(
                        "The daemon did not answer; check the sidebar before spawning again.",
                    ))),
                };
                Answer::Spawned { id, result }
            }
        };
        if answered.send(answer).is_err() {
            return;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Sidebar,
    View,
}

enum Overlay {
    None,
    Attention,
    Spawn(Box<spawn::Dialog>),
}

/// A confirmation waiting for its key to be pressed again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Armed {
    Quit,
    Close,
}

struct Console<'a> {
    root: PathBuf,
    args: &'a Tui,
    router: Router,
    sidebar: Sidebar,
    attention: Attention,
    layout: Layout,
    views: HashMap<String, View>,
    /// When views were closed; their long-polls may still be in flight.
    closed: Vec<Instant>,
    focus: Focus,
    overlay: Overlay,
    /// Where the next agent picked in the sidebar opens; by default its own tab.
    place: Option<Place>,
    status: String,
    connected: bool,
    /// The tree's long-poll waits for a viewer slot.
    waiting: bool,
    armed: Option<Armed>,
    /// Agents needing an answer, to announce after the next frame.
    notify: Option<usize>,
    quit: bool,
    /// Where the last frame drew each part, for the mouse.
    hits: pointer::Hits,
    asks: Sender<Ask>,
    answers: Receiver<Answer>,
    /// The models and efforts `tui.toml` lists, offered by views and the dialog.
    lists: Arc<Lists>,
}

impl<'a> Console<'a> {
    fn new(
        root: PathBuf,
        prefix: Prefix,
        args: &'a Tui,
        asks: Sender<Ask>,
        answers: Receiver<Answer>,
    ) -> Self {
        let mut console = Self {
            root,
            args,
            router: Router::new(prefix),
            sidebar: Sidebar::new(args.keep_ended),
            attention: Attention::default(),
            layout: Layout::default(),
            views: HashMap::new(),
            closed: Vec::new(),
            focus: Focus::Sidebar,
            overlay: Overlay::None,
            place: None,
            status: String::new(),
            connected: false,
            waiting: false,
            armed: None,
            notify: None,
            quit: false,
            hits: pointer::Hits::default(),
            asks,
            answers,
            lists: Arc::default(),
        };
        console.status = console.hint();
        console
    }

    fn prefix(&self) -> String {
        self.router.prefix().label()
    }

    fn hint(&self) -> String {
        let prefix = self.prefix();
        format!("{prefix} then: s sidebar · t tab · v split · c spawn · a attention · q quit")
    }

    fn mode(&self) -> Mode {
        Mode {
            read_only: false,
            exit_on_end: !self.args.keep_ended,
            linger: END_LINGER,
            lists: Arc::clone(&self.lists),
        }
    }

    fn run(
        &mut self,
        terminal: &mut ratatui::DefaultTerminal,
        feed: &Receiver<Feed>,
        signals: &AtomicUsize,
    ) -> Result<Option<Value>> {
        let mut dirty = true;
        loop {
            let now = Instant::now();
            while let Ok(update) = feed.try_recv() {
                if let Feed::Refused(reply) = update {
                    return Ok(Some(reply));
                }
                self.feed(update);
                dirty = true;
            }
            while let Ok(answer) = self.answers.try_recv() {
                self.answer(answer, now);
                dirty = true;
            }
            dirty |= self.pump(now);
            dirty |= self.router.expire(now);
            if signals.load(Ordering::Relaxed) > 0 || self.quit {
                return Ok(None);
            }
            if dirty {
                terminal
                    .draw(|frame| self.draw(frame))
                    .map_err(super::attach::terminal_error)?;
                dirty = false;
            }
            if let Some(waiting) = self.notify.take()
                && !self.args.quiet
            {
                // A lost notification is not worth ending the view for.
                let _ = attention::notify(&mut std::io::stdout(), waiting);
            }
            if event::poll(TICK).map_err(super::attach::terminal_error)? {
                let event = event::read().map_err(super::attach::terminal_error)?;
                dirty |= self.event(&event, Instant::now());
                for view in self.views.values_mut() {
                    if let Some(text) = view.take_clipboard() {
                        // The view's status line shows the text too.
                        let _ = super::attach::copy_to_clipboard(&mut std::io::stdout(), &text);
                    }
                }
            }
        }
    }
}

/// Draw the spawn dialog; returns its area and each field row, with the
/// column its line starts at.
fn draw_spawn(
    frame: &mut Frame<'_>,
    body: Rect,
    dialog: &spawn::Dialog,
) -> (Rect, Vec<(u16, u16, spawn::Field)>) {
    const WIDTH: u16 = 76;
    // The lines are laid out for the dialog's inner width, inside its border.
    let spawn::Rendered {
        lines,
        fields,
        cursor,
    } = dialog.lines(usize::from(WIDTH.min(body.width).saturating_sub(2)));
    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let area = centered(body, WIDTH, height);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title(" spawn an agent ")
        .border_style(Style::new().fg(Color::Yellow));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = fields
        .iter()
        .enumerate()
        .filter_map(|(index, field)| {
            let row = u16::try_from(index)
                .ok()
                .filter(|row| *row < inner.height)?;
            Some((inner.y + row, inner.x, (*field)?))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
    if let Some((row, column)) = cursor
        && let (Ok(row), Ok(column)) = (u16::try_from(row), u16::try_from(column))
        && row < inner.height
    {
        frame.set_cursor_position((
            inner.x + column.min(inner.width.saturating_sub(1)),
            inner.y + row,
        ));
    }
    (area, rows)
}

/// A box of at most `width` × `height` in the middle of `area`.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests;
