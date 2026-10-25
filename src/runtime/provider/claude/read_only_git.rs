//! Read-only agents may run one read-only `git` command. The `PreToolUse` hook
//! parses the agent's command strictly and replaces it with a quoted rendering,
//! so no shell syntax, environment change or option that writes files or runs
//! another program reaches the shell. The read-only sandbox still applies.
//! Programs a repository configures (for example filters) are not disabled;
//! they run under that sandbox.

/// Longest command the hook accepts.
const MAX_BYTES: usize = 8 * 1024;
/// Most words a command may have.
const MAX_WORDS: usize = 128;
/// Subcommands that only read the repository.
const READING: &[&str] = &[
    "status",
    "diff",
    "log",
    "show",
    "blame",
    "grep",
    "ls-files",
    "ls-tree",
    "rev-parse",
    "rev-list",
    "merge-base",
    "cat-file",
    "describe",
    "shortlog",
    "diff-tree",
    "for-each-ref",
    "name-rev",
];
/// Options that write a file, open a pager program or run a configured program.
/// Long options are refused by any prefix git would accept as an abbreviation.
const REFUSED_LONG: &[&str] = &[
    "output",
    "output-directory",
    "open-files-in-pager",
    "ext-diff",
    "textconv",
    "filters",
    "show-signature",
    "remerge-diff",
    "exec",
    "upload-pack",
];
/// Exact option names that also prefix a refused one: git reads an exact
/// name as itself, never as an abbreviation.
const EXACT_READING: &[&str] = &["text", "filter"];
/// `git branch` options that only list.
const BRANCH_LISTING: &[&str] = &[
    "--list",
    "-l",
    "--show-current",
    "-a",
    "--all",
    "-r",
    "--remotes",
    "-v",
    "-vv",
    "--verbose",
];
/// The environment every rendered command runs with: no optional index writes,
/// no pager, no credential prompt and no lazy fetch.
const ENVIRONMENT: &str =
    "GIT_OPTIONAL_LOCKS=0 GIT_PAGER=cat PAGER=cat GIT_TERMINAL_PROMPT=0 GIT_NO_LAZY_FETCH=1";

/// An accepted command: the subcommand and its arguments.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Git {
    words: Vec<String>,
}
impl Git {
    /// The command to run instead of the agent's: every word single-quoted. It
    /// execs git, so the exit status is git's own. Claude's shell wrapper would
    /// otherwise record the working directory in its temporary directory next,
    /// which the read-only sandbox refuses, turning every success into exit 1.
    /// The agent cannot change directory, so nothing needs that record.
    pub(crate) fn render(&self) -> String {
        let mut words: Vec<&str> = Vec::with_capacity(self.words.len() + 3);
        words.push(&self.words[0]);
        words.extend(guards(&self.words));
        words.extend(self.words[1..].iter().map(String::as_str));
        let quoted: Vec<String> = words.iter().map(|word| quote(word)).collect();
        format!(
            "{ENVIRONMENT} exec git --no-pager -c core.fsmonitor=false {}",
            quoted.join(" ")
        )
    }
}

/// Options that keep repository configuration from running external diff,
/// conversion or signature programs.
fn guards(words: &[String]) -> &'static [&'static str] {
    match (words[0].as_str(), words.get(1).map(String::as_str)) {
        ("log" | "show", _) => &["--no-ext-diff", "--no-textconv", "--no-show-signature"],
        ("diff" | "diff-tree", _) => &["--no-ext-diff", "--no-textconv"],
        ("blame" | "grep", _) => &["--no-textconv"],
        _ => &[],
    }
}
fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// Accept `command` only as one read-only git command.
pub(crate) fn parse(command: &str) -> Option<Git> {
    let mut words = split(command)?.into_iter();
    if words.next()? != "git" {
        return None;
    }
    let mut words: Vec<String> = words
        .skip_while(|word| word == "--no-pager" || word == "-P")
        .collect();
    let subcommand = words.first()?.clone();
    if subcommand.starts_with('-') {
        // Global options before the subcommand change where and how git runs.
        return None;
    }
    let options = words.iter().skip(1).take_while(|word| *word != "--");
    for word in options.clone() {
        if refused(word, &subcommand) {
            return None;
        }
    }
    let accepted = match subcommand.as_str() {
        "branch" => listing_branches(&words[1..]),
        "stash" => matches!(words.get(1).map(String::as_str), Some("list" | "show")),
        "worktree" => {
            words.get(1).map(String::as_str) == Some("list")
                && words[2..]
                    .iter()
                    .all(|word| word == "--porcelain" || word == "-v")
        }
        name => READING.contains(&name),
    };
    if !accepted {
        return None;
    }
    if words.len() >= 2 && subcommand == "stash" && words[1] == "show" {
        // Diff options follow the stash subcommand.
        words.splice(2..2, ["--no-ext-diff", "--no-textconv"].map(String::from));
    }
    Some(Git { words })
}

/// Only listing: options from the listing set, and patterns only with a list flag.
fn listing_branches(words: &[String]) -> bool {
    let listing = words.iter().any(|word| word == "--list" || word == "-l");
    words
        .iter()
        .all(|word| BRANCH_LISTING.contains(&word.as_str()) || (listing && !word.starts_with('-')))
}

fn refused(word: &str, subcommand: &str) -> bool {
    if verifies_signatures(word) {
        return true;
    }
    if let Some(long) = word.strip_prefix("--") {
        let (name, value) = long
            .split_once('=')
            .map_or((long, None), |(name, value)| (name, Some(value)));
        if name.is_empty() {
            return false;
        }
        // Remerge diffs run configured merge drivers.
        if "diff-merges".starts_with(name) && matches!(value, Some("remerge" | "r")) {
            return true;
        }
        // Verbose status shows diffs through configured conversion programs.
        if subcommand == "status" && "verbose".starts_with(name) {
            return true;
        }
        // An exact option name is never read as an abbreviation of another.
        if EXACT_READING.contains(&name) {
            return false;
        }
        return REFUSED_LONG.iter().any(|refused| refused.starts_with(name));
    }
    let Some(flags) = word.strip_prefix('-') else {
        return false;
    };
    let flags: Vec<char> = flags
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect();
    // grep -O opens matches in a pager program; status -v shows converted diffs.
    (subcommand == "grep" && flags.contains(&'O'))
        || (subcommand == "status" && flags.contains(&'v'))
}

/// Placeholders that verify signatures through the configured GPG program:
/// `%G…` with any `+`, `-` or space modifier, and `%(signature…)` including
/// its `*` dereference.
fn verifies_signatures(word: &str) -> bool {
    word.match_indices('%').any(|(at, _)| {
        let rest = &word[at + 1..];
        let rest = rest.strip_prefix(['+', '-', ' ']).unwrap_or(rest);
        rest.starts_with('G')
            || rest
                .strip_prefix('(')
                .is_some_and(|atom| atom.trim_start_matches('*').starts_with("signature"))
    })
}

/// Characters a word may contain outside quotes.
fn plain(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_./:@%+,=^~-".contains(c)
}
/// Split into words: plain characters, `'…'` without `'`, and `"…"` holding
/// only plain characters and spaces. Anything else is shell syntax.
fn split(command: &str) -> Option<Vec<String>> {
    if command.len() > MAX_BYTES || command.chars().any(char::is_control) {
        return None;
    }
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' => {
                if let Some(done) = word.take() {
                    words.push(done);
                }
            }
            '\'' => {
                let current = word.get_or_insert_with(String::new);
                loop {
                    match chars.next()? {
                        '\'' => break,
                        inner => current.push(inner),
                    }
                }
            }
            '"' => {
                let current = word.get_or_insert_with(String::new);
                loop {
                    match chars.next()? {
                        '"' => break,
                        inner if plain(inner) || inner == ' ' => current.push(inner),
                        _ => return None,
                    }
                }
            }
            c if plain(c) => word.get_or_insert_with(String::new).push(c),
            _ => return None,
        }
        if words.len() > MAX_WORDS {
            return None;
        }
    }
    words.extend(word);
    (words.len() <= MAX_WORDS).then_some(words)
}

#[cfg(test)]
mod tests;
