//! The tmux backend: windows and panes in the controller's own session. Commands
//! use tmux's argument form, so no shell ever parses an agent's command line.
use super::backend::{Backend, Failure, PaneCommand};
use super::plan::Pane;
use crate::error::Error;
use std::process::Command;
use std::time::Duration;

/// Agent panes per window, tiled 2×2.
pub(super) const PER_TAB: usize = 4;
/// Characters of tmux's stderr shown with a failed command.
const ERROR_TEXT: usize = 200;
/// One listed pane per line; the start command comes last, so a tab inside it
/// cannot shift the other fields.
const LIST_FORMAT: &str =
    "#{window_id}\t#{pane_id}\t#{pane_dead}\t#{pane_title}\t#{pane_start_command}";

/// tmux, reached through `tmux` with the server named by `TMUX`.
pub(super) struct Tmux {
    /// The controller's session, such as `$3`.
    session: String,
    /// The controller's own pane, such as `%0`.
    pane: String,
    /// The canonical state root every view this controller opens names.
    root: String,
    /// A problem after a successful open or listing, for the status line.
    notice: Option<String>,
}

/// One pane from the listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Listed {
    pub window: u64,
    pub pane: String,
    pub dead: bool,
    /// The agent of a `hivectl attach` view.
    pub agent: Option<String>,
    /// The view is one this controller would open: `--state-dir ROOT` and
    /// `--exit-on-end`.
    pub owned: bool,
}

impl Tmux {
    pub(super) fn new(session: String, pane: String, root: String) -> Self {
        Self {
            session,
            pane,
            root,
            notice: None,
        }
    }

    fn run(args: &[String]) -> Result<String, Failure> {
        let mut command = Command::new("tmux");
        command.args(args);
        let action = args.first().map_or("", String::as_str);
        let output = crate::os::output(&mut command).map_err(|cause| {
            if cause.kind() == std::io::ErrorKind::NotFound {
                Failure::Fatal(Error::pane_backend_unavailable(
                    "tmux was not found on PATH; install it or add it to PATH.",
                ))
            } else {
                Failure::Retry(format!("tmux {action} could not start ({})", cause.kind()))
            }
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail: String = stderr.trim().chars().take(ERROR_TEXT).collect();
            return Err(Failure::Retry(format!(
                "tmux {action} failed ({}): {detail}",
                output.status
            )));
        }
        String::from_utf8(output.stdout)
            .map_err(|_| Failure::Retry(format!("tmux {action} printed non-UTF-8 output")))
    }

    /// Keep a follow-up failure as a notice: what it followed has succeeded.
    fn keep(&mut self, result: Result<String, Failure>) -> Result<(), Failure> {
        match result {
            Ok(_) => Ok(()),
            Err(Failure::Retry(problem)) => {
                self.notice = Some(problem);
                Ok(())
            }
            Err(fatal) => Err(fatal),
        }
    }
}

impl Backend for Tmux {
    fn panes(&mut self) -> Result<Vec<Pane>, Failure> {
        let listing = parse_listing(&Self::run(&list_argv(&self.session))?, &self.root);
        let mut panes = Vec::new();
        for listed in listing {
            if listed.dead {
                // Only a dead view this controller opened is its own to close.
                if listed.owned {
                    let closed = Self::run(&kill_argv(&listed.pane));
                    self.keep(closed)?;
                }
                continue;
            }
            let Some(agent) = listed.agent else {
                continue;
            };
            panes.push(Pane {
                agent,
                tab: listed.window,
            });
        }
        Ok(panes)
    }
    fn open_tab(&mut self, name: &str, pane: &PaneCommand) -> Result<u64, Failure> {
        let output = Self::run(&new_window_argv(&self.session, name, pane))?;
        let window = window_key(output.trim())
            .ok_or_else(|| Failure::Retry(String::from("tmux new-window printed no window ID")))?;
        let remain = Self::run(&remain_argv(window));
        self.keep(remain)?;
        Ok(window)
    }
    fn open_pane(&mut self, tab: u64, pane: &PaneCommand, _stacked: bool) -> Result<(), Failure> {
        Self::run(&split_argv(tab, pane))?;
        let tiled = Self::run(&layout_argv(tab));
        self.keep(tiled)
    }
    fn announce(&mut self) -> Result<(), Failure> {
        Self::run(&announce_argv(&self.pane)).map(drop)
    }
    fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
    }
    fn attach_flags(&self) -> &'static [&'static str] {
        &["--title"]
    }
    fn layout(&self) -> (usize, bool) {
        (PER_TAB, false)
    }
    fn list_every(&self) -> Duration {
        LIST_INTERVAL
    }
}

/// The session and socket path of the controller's pane, or `None` when tmux
/// does not answer.
pub(super) fn session(pane: &str) -> Option<(String, String)> {
    let text = Tmux::run(&session_argv(pane)).ok()?;
    let (session, socket) = text.trim_end_matches('\n').split_once('\t')?;
    (!session.is_empty() && !socket.is_empty()).then(|| (session.to_owned(), socket.to_owned()))
}

/// The terminal device of a pane, such as `/dev/ttys004`.
pub(super) fn pane_tty(pane: &str) -> Option<String> {
    let text = Tmux::run(&tty_argv(pane)).ok()?;
    let tty = text.trim_end_matches('\n');
    (!tty.is_empty()).then(|| tty.to_owned())
}

/// An argument as tmux must receive it: a trailing `;` would end the command,
/// so it is escaped as `\;`, which tmux turns back into `;`.
pub(super) fn escape(argument: &str) -> String {
    match argument.strip_suffix(';') {
        Some(head) => format!("{head}\\;"),
        None => argument.to_owned(),
    }
}

fn argv(words: &[&str], pane: Option<&PaneCommand>) -> Vec<String> {
    let mut argv: Vec<String> = words.iter().map(|word| escape(word)).collect();
    if let Some(pane) = pane {
        argv.push(String::from("--"));
        argv.push(escape(&pane.program.to_string_lossy()));
        argv.extend(pane.args.iter().map(|arg| escape(arg)));
    }
    argv
}

pub(super) fn list_argv(session: &str) -> Vec<String> {
    argv(
        &["list-panes", "-s", "-t", session, "-F", LIST_FORMAT],
        None,
    )
}

pub(super) fn new_window_argv(session: &str, name: &str, pane: &PaneCommand) -> Vec<String> {
    let target = format!("{session}:");
    argv(
        &[
            "new-window",
            "-d",
            "-P",
            "-F",
            "#{window_id}",
            "-t",
            &target,
            "-n",
            name,
        ],
        Some(pane),
    )
}

pub(super) fn split_argv(window: u64, pane: &PaneCommand) -> Vec<String> {
    let target = format!("@{window}");
    argv(
        &[
            "split-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-t",
            &target,
        ],
        Some(pane),
    )
}

pub(super) fn layout_argv(window: u64) -> Vec<String> {
    argv(
        &["select-layout", "-t", &format!("@{window}"), "tiled"],
        None,
    )
}

pub(super) fn remain_argv(window: u64) -> Vec<String> {
    argv(
        &[
            "set-option",
            "-w",
            "-t",
            &format!("@{window}"),
            "remain-on-exit",
            "off",
        ],
        None,
    )
}

pub(super) fn kill_argv(pane: &str) -> Vec<String> {
    argv(&["kill-pane", "-t", pane], None)
}

pub(super) fn announce_argv(pane: &str) -> Vec<String> {
    argv(&["select-pane", "-t", pane, "-T", "hive:panes"], None)
}

pub(super) fn session_argv(pane: &str) -> Vec<String> {
    argv(
        &[
            "display-message",
            "-p",
            "-t",
            pane,
            "#{session_id}\t#{socket_path}",
        ],
        None,
    )
}

pub(super) fn tty_argv(pane: &str) -> Vec<String> {
    argv(&["display-message", "-p", "-t", pane, "#{pane_tty}"], None)
}

/// A window ID such as `@12`, as the reconciler's tab key.
pub(super) fn window_key(id: &str) -> Option<u64> {
    id.strip_prefix('@')?.parse().ok()
}

/// Panes in `list-panes` output; lines that do not fit the format are skipped.
/// `root` is the canonical state root this controller's views name.
pub(super) fn parse_listing(text: &str, root: &str) -> Vec<Listed> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(5, '\t');
            let window = window_key(fields.next()?)?;
            let pane = fields.next().filter(|pane| pane.starts_with('%'))?;
            let dead = match fields.next()? {
                "0" => false,
                "1" => true,
                _ => return None,
            };
            let title = fields.next()?;
            let command = words(fields.next().unwrap_or_default());
            let started = attached(&command);
            let owned = started.is_some() && owned(&command, root);
            Some(Listed {
                window,
                pane: pane.to_owned(),
                dead,
                // Navigation changes the title, but never the start command.
                // Before the view starts, or for an untitled manual view,
                // its start command identifies it without waiting for a title.
                agent: title
                    .strip_prefix("hive:")
                    .filter(|id| uuid::Uuid::parse_str(id).is_ok())
                    .map(str::to_owned)
                    .or(started),
                owned,
            })
        })
        .collect()
}

/// The agent ID following `attach` in a start command's words. Words
/// containing whitespace are split again, so a view started through a shell
/// (`'hivectl attach ID'`) is recognized too.
pub(super) fn attached(words: &[String]) -> Option<String> {
    let flat: Vec<&str> = words
        .iter()
        .flat_map(|word| word.split_whitespace())
        .collect();
    flat.windows(2)
        .find(|pair| pair[0] == "attach" && uuid::Uuid::parse_str(pair[1]).is_ok())
        .map(|pair| pair[1].to_owned())
}

/// The command is one this controller builds: its exact `--state-dir ROOT`
/// pair and `--exit-on-end`.
pub(super) fn owned(words: &[String], root: &str) -> bool {
    words
        .windows(2)
        .any(|pair| pair[0] == "--state-dir" && pair[1] == root)
        && words.iter().any(|word| word == "--exit-on-end")
}

/// Shell-style words, as tmux quotes a start command. Quotes group a word;
/// inside and outside them tmux escapes C-style (`\\`, `\"`, `\$`, `\t`,
/// `\ooo`), so a backslash always starts an escape.
pub(super) fn words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            quote @ ('\'' | '"') => {
                started = true;
                while let Some(inner) = characters.next() {
                    match inner {
                        _ if inner == quote => break,
                        '\\' => unescape(&mut characters, &mut word),
                        other => word.push(other),
                    }
                }
            }
            '\\' => {
                started = true;
                unescape(&mut characters, &mut word);
            }
            space if space.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            other => {
                started = true;
                word.push(other);
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

/// The character after a backslash: a C-style escape, an octal byte, or the
/// character itself.
fn unescape(characters: &mut std::iter::Peekable<std::str::Chars<'_>>, word: &mut String) {
    let Some(escaped) = characters.next() else {
        return;
    };
    match escaped {
        't' => word.push('\t'),
        'n' => word.push('\n'),
        'r' => word.push('\r'),
        digit @ '0'..='7' => {
            let mut value = digit.to_digit(8).unwrap_or_default();
            for _ in 0..2 {
                let Some(next) = characters.peek().and_then(|next| next.to_digit(8)) else {
                    break;
                };
                value = value * 8 + next;
                characters.next();
            }
            word.extend(u8::try_from(value).ok().map(char::from));
        }
        other => word.push(other),
    }
}

#[cfg(test)]
mod tests;

const LIST_INTERVAL: std::time::Duration = Duration::from_secs(2);
