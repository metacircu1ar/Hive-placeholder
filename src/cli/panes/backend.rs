//! The pane backend: zellij, driven through its CLI with checked subprocesses.
//! Arguments and layouts are built as data, so they are tested without zellij.
use super::plan::Pane;
use serde_json::Value;
use std::fmt::Write;
use std::path::PathBuf;
use std::process::Command;

/// Characters of zellij's stderr shown with a failed command.
const ERROR_TEXT: usize = 200;

/// The command one agent's pane runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PaneCommand {
    pub agent: String,
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl PaneCommand {
    fn name(&self) -> String {
        format!("hive:{}", self.agent)
    }
}

/// Why a backend command failed.
#[derive(Debug)]
pub(super) enum Failure {
    /// Shown in the status list; the next pass tries again.
    Retry(String),
    /// The backend cannot work at all; the controller ends with this error.
    Fatal(crate::error::Error),
}

pub(super) trait Backend {
    /// Agent panes the backend can see, with their tabs.
    fn panes(&mut self) -> Result<Vec<Pane>, Failure>;
    /// Open a tab holding this pane; returns the new tab's key.
    fn open_tab(&mut self, name: &str, pane: &PaneCommand) -> Result<u64, Failure>;
    fn open_pane(&mut self, tab: u64, pane: &PaneCommand, stacked: bool) -> Result<(), Failure>;
    /// Mark the controller's own pane; a failure only shows in the status list.
    fn announce(&mut self) -> Result<(), Failure> {
        Ok(())
    }
    /// A problem after a successful open or listing, shown in the status line
    /// without counting as a failed open.
    fn take_notice(&mut self) -> Option<String> {
        None
    }
    /// Extra `hivectl attach` arguments this backend needs to identify its panes.
    fn attach_flags(&self) -> &'static [&'static str] {
        &[]
    }
    /// Agent panes per tab, and whether they stack rather than split.
    fn layout(&self) -> (usize, bool);
    /// How often panes are listed when nothing else wakes the controller.
    fn list_every(&self) -> std::time::Duration;
}

/// zellij, reached through `zellij action` in the current session.
pub(super) struct Zellij;

impl Zellij {
    fn run(args: &[String]) -> Result<String, Failure> {
        let mut command = Command::new("zellij");
        command.arg("action").args(args);
        let action = args.first().map_or("", String::as_str);
        let output = crate::os::output(&mut command).map_err(|cause| {
            if cause.kind() == std::io::ErrorKind::NotFound {
                Failure::Fatal(crate::error::Error::pane_backend_unavailable(
                    "zellij was not found on PATH; install it or add it to PATH.",
                ))
            } else {
                Failure::Retry(format!(
                    "zellij {action} could not start ({})",
                    cause.kind()
                ))
            }
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail: String = stderr.trim().chars().take(ERROR_TEXT).collect();
            return Err(Failure::Retry(format!(
                "zellij {action} failed ({}): {detail}",
                output.status
            )));
        }
        String::from_utf8(output.stdout)
            .map_err(|_| Failure::Retry(format!("zellij {action} printed non-UTF-8 output")))
    }
}

impl Backend for Zellij {
    fn panes(&mut self) -> Result<Vec<Pane>, Failure> {
        parse_panes(&Self::run(&list_panes_argv())?).map_err(Failure::Retry)
    }
    fn open_tab(&mut self, name: &str, pane: &PaneCommand) -> Result<u64, Failure> {
        let output = Self::run(&new_tab_argv(name, pane))?;
        output
            .trim()
            .parse()
            .map_err(|_| Failure::Retry(String::from("zellij new-tab printed no tab ID")))
    }
    fn open_pane(&mut self, tab: u64, pane: &PaneCommand, stacked: bool) -> Result<(), Failure> {
        Self::run(&new_pane_argv(tab, pane, stacked)).map(drop)
    }
    fn announce(&mut self) -> Result<(), Failure> {
        let Ok(pane) = std::env::var("ZELLIJ_PANE_ID") else {
            return Ok(());
        };
        Self::run(&[
            String::from("rename-pane"),
            String::from("--pane-id"),
            format!("terminal_{pane}"),
            String::from("hive:panes"),
        ])
        .map(drop)
    }
    fn layout(&self) -> (usize, bool) {
        (super::plan::ZELLIJ_PER_TAB, true)
    }
    fn list_every(&self) -> std::time::Duration {
        LIST_INTERVAL
    }
}

pub(super) fn list_panes_argv() -> Vec<String> {
    ["list-panes", "--json", "--all"].map(String::from).to_vec()
}

pub(super) fn new_tab_argv(name: &str, pane: &PaneCommand) -> Vec<String> {
    [
        "new-tab",
        "--no-focus",
        "--name",
        name,
        "--layout-string",
        &layout(pane),
    ]
    .map(String::from)
    .to_vec()
}

pub(super) fn new_pane_argv(tab: u64, pane: &PaneCommand, stacked: bool) -> Vec<String> {
    let mut argv: Vec<String> = [
        "new-pane",
        "--tab-id",
        &tab.to_string(),
        "--name",
        &pane.name(),
    ]
    .map(String::from)
    .to_vec();
    argv.extend(["--close-on-exit", "--no-focus"].map(String::from));
    if stacked {
        argv.push(String::from("--stacked"));
    }
    // The program and its arguments are separate argv entries, never a shell line.
    argv.push(String::from("--"));
    argv.push(pane.program.to_string_lossy().into_owned());
    argv.extend(pane.args.iter().cloned());
    argv
}

/// A tab with zellij's tab bar, the agent's pane and the status bar. The tab
/// closes with the agent's pane, its only terminal pane.
pub(super) fn layout(pane: &PaneCommand) -> String {
    let args: Vec<String> = pane.args.iter().map(|arg| kdl(arg)).collect();
    format!(
        "layout {{ pane size=1 borderless=true {{ plugin location=\"tab-bar\"; }}; \
         pane command={} name={} close_on_exit=true {{ args {}; }}; \
         pane size=2 borderless=true {{ plugin location=\"status-bar\"; }}; }}",
        kdl(&pane.program.to_string_lossy()),
        kdl(&pane.name()),
        args.join(" ")
    )
}

/// A KDL string literal.
pub(super) fn kdl(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            control if control.is_control() => {
                // Writing to a String cannot fail.
                let _ = write!(quoted, "\\u{{{:x}}}", u32::from(control));
            }
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// Agent panes in `list-panes --json --all` output: named exactly
/// `hive:<agent UUID>`, or running `attach <agent UUID>` if renamed.
pub(super) fn parse_panes(json: &str) -> Result<Vec<Pane>, String> {
    let panes: Vec<Value> = serde_json::from_str(json)
        .map_err(|_| String::from("zellij list-panes printed invalid JSON"))?;
    Ok(panes
        .iter()
        .map(crate::cli::views::read::<crate::cli::views::TerminalPane<'_>>)
        .filter(|pane| pane.is_plugin == Some(false) && pane.exited != Some(true))
        .filter_map(|pane| {
            let agent = pane
                .title
                .and_then(|title| title.strip_prefix("hive:"))
                .filter(|id| uuid::Uuid::parse_str(id).is_ok())
                .map(str::to_owned)
                .or_else(|| attached(pane.terminal_command?))?;
            Some(Pane {
                agent,
                tab: pane.tab_id?,
            })
        })
        .collect())
}

/// The agent ID following `attach` in a pane's command line.
fn attached(command: &str) -> Option<String> {
    let mut words = command.split_whitespace();
    words.find(|word| *word == "attach")?;
    words
        .next()
        .filter(|id| uuid::Uuid::parse_str(id).is_ok())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests;

const LIST_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
