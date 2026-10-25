//! The Ghostty backend (macOS): fixed scripts run by `osascript`, with every
//! value passed through `argv`, never interpolated into the script.
use super::backend::{Backend, Failure, PaneCommand};
use super::plan::Pane;
use crate::error::Error;
use std::collections::HashMap;
use std::process::Command;
use std::time::Duration;

/// Agent terminals per tab: a 2×2 grid of splits.
pub(super) const PER_TAB: usize = 4;
/// The prefix of the unique title the controller gives its own terminal; the
/// terminal with that title locates the controller's window (Ghostty reports
/// no tty), and reading it back checks that titles reach Ghostty.
pub(super) const CONTROLLER_TITLE: &str = "hive:panes:";
/// Characters of `osascript` stderr shown with a failed command.
const ERROR_TEXT: usize = 200;
const FIELD: char = '\u{1f}';
const ROW: char = '\u{1e}';

/// Every terminal: window, tab, terminal ID and title, as separated text.
const LIST: &str = "on run argv\n\tset fieldSep to (character id 31)\n\tset rowSep to \
    (character id 30)\n\tset out to \"\"\n\ttell application id \
    \"com.mitchellh.ghostty\"\n\t\trepeat with w in windows\n\t\t\tset \
    windowId to id of w\n\t\t\trepeat with t in tabs of w\n\t\t\t\tset tabId \
    to id of t\n\t\t\t\trepeat with term in terminals of t\n\t\t\t\t\tset out \
    to out & windowId & fieldSep & tabId & fieldSep & (id of term) & fieldSep \
    & (name of term) & rowSep\n\t\t\t\tend repeat\n\t\t\tend repeat\n\t\tend \
    repeat\n\tend tell\n\treturn out\nend run";

/// A new tab in the controller's window, titled, without moving the user's
/// focus: the front window's terminal and that window's tab are restored.
const NEW_TAB: &str = r#"on run argv
	set windowId to item 1 of argv
	set commandText to item 2 of argv
	set tabTitle to item 3 of argv
	tell application id "com.mitchellh.ghostty"
		set frontTerminal to missing value
		try
			set frontTerminal to focused terminal of selected tab of front window
		end try
		set w to first window whose id is windowId
		set previousTab to selected tab of w
		set cfg to new surface configuration
		set command of cfg to commandText
		set wait after command of cfg to false
		set newTab to new tab in w with configuration cfg
		set newTerminal to terminal 1 of newTab
		perform action ("set_tab_title:" & tabTitle) on newTerminal
		try
			select tab previousTab
			if frontTerminal is not missing value then focus frontTerminal
		end try
		return (id of newTab) & (character id 31) & (id of newTerminal)
	end tell
end run"#;

/// A split of an agent terminal, equalised, without moving the user's focus.
const SPLIT: &str = r#"on run argv
	set targetId to item 1 of argv
	set directionText to item 2 of argv
	set commandText to item 3 of argv
	tell application id "com.mitchellh.ghostty"
		set target to first terminal whose id is targetId
		set frontTerminal to missing value
		try
			set frontTerminal to focused terminal of selected tab of front window
		end try
		set cfg to new surface configuration
		set command of cfg to commandText
		set wait after command of cfg to false
		if directionText is "right" then
			set newTerminal to split target direction right with configuration cfg
		else
			set newTerminal to split target direction down with configuration cfg
		end if
		perform action "equalize_splits" on newTerminal
		try
			if frontTerminal is not missing value then focus frontTerminal
		end try
		return id of newTerminal
	end tell
end run"#;

/// One terminal from the listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Terminal {
    pub window: String,
    pub tab: String,
    pub id: String,
    pub name: String,
}

impl Terminal {
    /// The agent of an agent view: a title exactly `hive:<UUID>`.
    fn agent(&self) -> Option<&str> {
        self.name
            .strip_prefix("hive:")
            .filter(|id| uuid::Uuid::parse_str(id).is_ok())
    }
}

/// Ghostty reached through `osascript`, from a controller in one of its terminals.
pub(super) struct Ghostty {
    /// The controller's own unique title, which locates its window.
    title: String,
    window: Option<String>,
    listing: Vec<Terminal>,
    /// Ghostty's text tab IDs, as the reconciler's numeric keys.
    keys: HashMap<String, u64>,
    ids: HashMap<u64, String>,
}

impl Ghostty {
    pub(super) fn new(title: String) -> Self {
        Self {
            title,
            window: None,
            listing: Vec::new(),
            keys: HashMap::new(),
            ids: HashMap::new(),
        }
    }

    fn key(&mut self, tab: &str) -> u64 {
        if let Some(key) = self.keys.get(tab) {
            return *key;
        }
        let key = u64::try_from(self.keys.len()).unwrap_or(u64::MAX);
        self.keys.insert(tab.to_owned(), key);
        self.ids.insert(key, tab.to_owned());
        key
    }

    /// Every terminal, and the controller's own window.
    pub(super) fn list(&mut self) -> Result<Vec<Terminal>, Failure> {
        let terminals = parse_listing(&run(LIST, &[])?);
        self.window = terminals
            .iter()
            .find(|terminal| terminal.name == self.title)
            .map(|terminal| terminal.window.clone());
        self.listing.clone_from(&terminals);
        Ok(terminals)
    }
}

impl Backend for Ghostty {
    fn panes(&mut self) -> Result<Vec<Pane>, Failure> {
        let terminals = self.list()?;
        Ok(terminals
            .iter()
            .filter_map(|terminal| {
                let agent = terminal.agent()?.to_owned();
                Some((agent, terminal.tab.clone()))
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|(agent, tab)| Pane {
                agent,
                tab: self.key(&tab),
            })
            .collect())
    }
    fn open_tab(&mut self, name: &str, pane: &PaneCommand) -> Result<u64, Failure> {
        let window = self.window.clone().ok_or_else(|| {
            Failure::Retry(String::from(
                "the controller's Ghostty window was not found",
            ))
        })?;
        let reply = run(NEW_TAB, &new_tab_args(&window, pane, name))?;
        let mut fields = reply.trim_end().split(FIELD);
        let (Some(tab), Some(id)) = (fields.next(), fields.next()) else {
            return Err(Failure::Retry(String::from("Ghostty returned no tab")));
        };
        // Later splits in this pass must see the new terminal.
        self.listing.push(Terminal {
            window,
            tab: tab.to_owned(),
            id: id.to_owned(),
            name: format!("hive:{}", pane.agent),
        });
        Ok(self.key(tab))
    }
    fn open_pane(&mut self, tab: u64, pane: &PaneCommand, _stacked: bool) -> Result<(), Failure> {
        let tab = self
            .ids
            .get(&tab)
            .cloned()
            .ok_or_else(|| Failure::Retry(String::from("the tab is gone")))?;
        let (target, direction) = split_target(&self.listing, &tab).ok_or_else(|| {
            Failure::Retry(String::from("the tab has no agent terminal to split"))
        })?;
        let window = self
            .listing
            .iter()
            .find(|terminal| terminal.id == target)
            .map(|terminal| terminal.window.clone())
            .unwrap_or_default();
        let id = run(SPLIT, &split_args(&target, direction, pane))?;
        self.listing.push(Terminal {
            window,
            tab,
            id: id.trim_end().to_owned(),
            name: format!("hive:{}", pane.agent),
        });
        Ok(())
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

pub(super) fn new_tab_args(window: &str, pane: &PaneCommand, name: &str) -> [String; 3] {
    [window.to_owned(), shell_command(pane), name.to_owned()]
}

pub(super) fn split_args(target: &str, direction: &str, pane: &PaneCommand) -> [String; 3] {
    [target.to_owned(), direction.to_owned(), shell_command(pane)]
}

/// The 2×2 grid: the second terminal splits the first to the right, the third
/// splits the first down, the fourth splits the second down.
pub(super) fn split_target(listing: &[Terminal], tab: &str) -> Option<(String, &'static str)> {
    let agents: Vec<&Terminal> = listing
        .iter()
        .filter(|terminal| terminal.tab == tab && terminal.agent().is_some())
        .collect();
    let (index, direction) = match agents.len() {
        1 => (0, "right"),
        2 => (0, "down"),
        _ => (1, "down"),
    };
    agents
        .get(index)
        .map(|terminal| (terminal.id.clone(), direction))
}

pub(super) fn parse_listing(text: &str) -> Vec<Terminal> {
    text.trim_end_matches(['\n', '\r'])
        .split(ROW)
        .filter_map(|row| {
            let mut fields = row.split(FIELD);
            Some(Terminal {
                window: fields.next()?.to_owned(),
                tab: fields.next()?.to_owned(),
                id: fields.next()?.to_owned(),
                name: fields.next()?.to_owned(),
            })
        })
        .collect()
}

/// The pane's command for `/bin/bash -c`: every argument single-quoted.
pub(super) fn shell_command(pane: &PaneCommand) -> String {
    std::iter::once(pane.program.to_string_lossy().into_owned())
        .chain(pane.args.iter().cloned())
        .map(|word| format!("'{}'", word.replace('\'', r"'\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Run one script with its arguments; refusals of Automation or of scripting
/// end the controller, other failures are retried on the next pass.
fn run(script: &str, args: &[String]) -> Result<String, Failure> {
    let mut command = Command::new("osascript");
    command.arg("-e").arg(script).arg("--").args(args);
    let output = crate::os::output(&mut command)
        .map_err(|cause| Failure::Retry(format!("osascript could not start ({})", cause.kind())))?;
    if output.status.success() {
        return String::from_utf8(output.stdout)
            .map_err(|_| Failure::Retry(String::from("osascript printed non-UTF-8 output")));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(classify(error_code(&stderr), &stderr))
}

/// The script error number in `osascript` stderr, such as `(-1743)`.
pub(super) fn error_code(stderr: &str) -> Option<i32> {
    let end = stderr.rfind(')')?;
    let start = stderr[..end].rfind("(-")?;
    stderr[start + 1..end].parse().ok()
}

pub(super) fn classify(code: Option<i32>, stderr: &str) -> Failure {
    match code {
        Some(-1743 | -1744) => Failure::Fatal(Error::pane_backend_unavailable(
            "macOS denied Automation for Ghostty; allow it in System Settings, Privacy \
                & Security, Automation, or use zellij or tmux.",
        )),
        Some(-1708 | -2741 | -2753) => Failure::Fatal(Error::pane_backend_unavailable(
            "Ghostty panes need Ghostty 1.3 or newer with macos-applescript enabled.",
        )),
        _ => {
            let detail: String = stderr.trim().chars().take(ERROR_TEXT).collect();
            Failure::Retry(format!("Ghostty script failed: {detail}"))
        }
    }
}

#[cfg(test)]
mod tests;

const LIST_INTERVAL: std::time::Duration = Duration::from_secs(5);
