//! Configure the native Claude process and its owned shell guard.
use super::wire::settings;
use crate::launch::LauncherCache;
use crate::runtime::api;
use crate::{
    config::Sandbox,
    error::{Error, Result},
    shell::guard::ShellGuard,
};
use serde_json::Value;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::Component;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// What a Claude session may run through its Bash tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shell {
    /// No shell at all (the steering canary).
    None,
    /// One read-only git command at a time (the read-only sandbox).
    ReadOnlyGit,
    /// Any command, inside the workspace-write sandbox.
    Full,
    /// Foreground commands without native filesystem or network confinement.
    Unrestricted,
}

#[derive(Clone)]
pub(crate) struct SessionConfig {
    pub tool_catalog: crate::compatibility::AgentCatalog,
    pub comb_reviewer: Option<bool>,
    pub mcp_servers: crate::config::McpServers,
    pub id: String,
    pub history: History,
    pub model: String,
    pub effort: String,
    pub options: std::sync::Arc<crate::options::Snapshot>,
    pub cwd: PathBuf,
    pub instructions: String,
    pub sandbox: Sandbox,
    pub steering: Option<super::SteeringPermit>,
    pub capability: Option<crate::runtime::steering_canary::Candidate>,
}

#[derive(Clone, Copy)]
pub(crate) enum IdentityPrompt {
    NativeSnapshot,
    CurrentAgent,
}
#[derive(Clone)]
pub(crate) enum History {
    New(IdentityPrompt),
    Resume(IdentityPrompt),
    Fork { source: String, cutoff: String },
}
impl History {
    pub(crate) fn fork_source(&self) -> Option<&str> {
        match self {
            Self::Fork { source, .. } => Some(source),
            Self::New(_) | Self::Resume(_) => None,
        }
    }
    fn current_identity(&self) -> bool {
        matches!(
            self,
            Self::Fork { .. }
                | Self::New(IdentityPrompt::CurrentAgent)
                | Self::Resume(IdentityPrompt::CurrentAgent)
        )
    }
}

impl History {
    fn configure(&self, id: &str, command: &mut Command) {
        if self.current_identity() {
            command.args(["--system-prompt-snapshot", "off"]);
        }
        match self {
            History::Fork { source, .. } => {
                command.args(["--resume", source, "--fork-session", "--session-id", id]);
            }
            History::New(_) => {
                command.args(["--session-id", id]);
            }
            History::Resume(_) => {
                command.args(["--resume", id]);
            }
        }
    }
}
impl SessionConfig {
    #[cfg(test)]
    pub(crate) fn for_test(id: impl Into<String>, cwd: PathBuf) -> Self {
        Self {
            tool_catalog: hive_input::compatibility::Exact,
            comb_reviewer: None,
            mcp_servers: crate::config::McpServers::default(),
            id: id.into(),
            cwd,
            history: History::New(IdentityPrompt::NativeSnapshot),
            model: "sonnet".into(),
            effort: "low".into(),
            options: crate::options::Snapshot::default().into(),
            instructions: "test".into(),
            sandbox: Sandbox::ReadOnly,
            steering: None,
            capability: None,
        }
    }

    /// What the session may run through Bash.
    pub(crate) fn shell(&self) -> Shell {
        if matches!(self.steering, Some(super::SteeringPermit::Canary)) {
            Shell::None
        } else if self.sandbox == Sandbox::ReadOnly {
            Shell::ReadOnlyGit
        } else if self.sandbox == Sandbox::FullAccess {
            Shell::Unrestricted
        } else {
            Shell::Full
        }
    }
    /// Bundled Claude skills through the `Skill` tool (`claude.skills`). The
    /// steering canary never has them.
    pub(crate) fn skills_enabled(&self) -> bool {
        !matches!(self.steering, Some(super::SteeringPermit::Canary))
            && self.options.values.get("claude.skills") == Some(&Value::Bool(true))
    }
    /// The native tools the session may call.
    fn tools(&self) -> String {
        let mut tools = String::from(match self.shell() {
            Shell::None => "Read",
            Shell::Full | Shell::Unrestricted => "Bash,Read,Glob,Grep,Edit,Write,NotebookEdit",
            Shell::ReadOnlyGit => "Bash,Read,Glob,Grep",
        });
        if self.skills_enabled() {
            tools.push_str(",Skill");
        }
        tools
    }
    pub(crate) fn configure(
        &self,
        command: &mut Command,
        launcher: &LauncherCache,
    ) -> Result<ShellGuard> {
        if self.sandbox != Sandbox::FullAccess
            && self
                .cwd
                .canonicalize()
                .map_err(Error::state_io_from)?
                .components()
                .any(|part| protected(part.as_os_str()))
        {
            return Err(Error::invalid_path(
                "Claude workspace cannot be inside protected metadata.",
            ));
        }
        let guard = ShellGuard::create(&self.cwd, &self.instructions, || launcher.executable())?;
        self.configure_arguments(command);
        guard.configure(command);
        self.history.configure(&self.id, command);
        let settings = self.sandbox_settings(&guard, launcher)?;
        command.arg("--settings").arg(settings.to_string());
        command.env("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", "1");
        command.env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1");
        command.env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
        self.configure_environment(command);
        Ok(guard)
    }

    fn configure_environment(&self, command: &mut Command) {
        if let Some(tokens) = self
            .options
            .values
            .get("claude.max_output_tokens")
            .and_then(Value::as_u64)
        {
            command.env("CLAUDE_CODE_MAX_OUTPUT_TOKENS", tokens.to_string());
        }
        if let Some(tokens) = self
            .options
            .values
            .get("claude.max_mcp_output_tokens")
            .and_then(Value::as_u64)
        {
            command.env("MAX_MCP_OUTPUT_TOKENS", tokens.to_string());
        }
        if let Some(tokens) = self.options.values.get("thinking").and_then(Value::as_u64) {
            command.env("CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING", "1");
            command.env("MAX_THINKING_TOKENS", tokens.to_string());
        }
    }

    fn configure_arguments(&self, command: &mut Command) {
        let mut servers = api::value(&self.mcp_servers);
        servers["hive"] = api::value(SdkServer {
            kind: "sdk",
            name: "hive",
        });
        let mcp = api::value(McpConfiguration { servers }).to_string();
        let allowed = std::iter::once("mcp__hive__*".to_owned())
            .chain(
                self.mcp_servers
                    .keys()
                    .map(|name| format!("mcp__{name}__*")),
            )
            .collect::<Vec<_>>()
            .join(",");
        // CLI settings sources are already empty in the profile command factory.
        // Do not use --bare: it disables subscription authentication.
        command.current_dir(&self.cwd).args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--replay-user-messages",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
            "--strict-mcp-config",
            "--mcp-config",
            &mcp,
        ]);
        // Slash commands also carry bundled skills, so they stay off unless the
        // agent opted in to skills.
        if !self.skills_enabled() {
            command.arg("--disable-slash-commands");
        }
        command.args([
            "--no-chrome",
            "--tools",
            &self.tools(),
            "--permission-mode",
            "default",
            "--allowedTools",
            &allowed,
            "--model",
            &self.model,
        ]);
        // Effort names come from the provider catalog and pass through unchanged;
        // only its literal "none" sentinel suppresses the native CLI option.
        if self.effort != "none" {
            command.args(["--effort", &self.effort]);
        }
    }
    fn sandbox_settings(&self, guard: &ShellGuard, launcher: &LauncherCache) -> Result<Value> {
        let root = guard.directory();
        let socket = root.join("socket");
        let unrestricted = self.sandbox == Sandbox::FullAccess;
        let filesystem = if unrestricted {
            None
        } else {
            Some(self.restricted_filesystem(guard, launcher)?)
        };
        Ok(api::value(settings::Settings {
            sandbox: settings::Sandbox {
                enabled: !unrestricted,
                fail_if_unavailable: !unrestricted,
                bash: settings::BashPolicy {
                    auto_allow_bash_if_sandboxed: !unrestricted,
                    allow_unsandboxed_commands: unrestricted,
                },
                excluded_commands: [],
                network: (!unrestricted).then_some(settings::Network {
                    allow_unix_sockets: [socket],
                    allow_all_unix_sockets: false,
                }),
                filesystem,
            },
            permissions: settings::Permissions {
                deny: [
                    "Agent",
                    "Task",
                    "TaskOutput",
                    "TaskStop",
                    "CronCreate",
                    "CronDelete",
                    "CronList",
                ],
            },
            auto_memory_enabled: false,
            always_thinking_enabled: self.options.values.get("thinking").and_then(|value| {
                value.as_u64().map(|_| true).or_else(|| {
                    value.as_str().and_then(|value| match value {
                        "on" => Some(true),
                        "off" => Some(false),
                        _ => None,
                    })
                })
            }),
            auto_compact_window: self
                .options
                .values
                .get("claude.auto_compact_window")
                .and_then(Value::as_u64),
        }))
    }

    fn restricted_filesystem(
        &self,
        guard: &ShellGuard,
        launcher: &LauncherCache,
    ) -> Result<settings::Filesystem> {
        let launcher_root = launcher.protected_root()?;
        literal_path(&launcher_root)?;
        let mut deny = vec![guard.protected_directory(), launcher_root];
        literal_path(&self.cwd)?;
        deny.extend(PROTECTED.iter().map(|name| self.cwd.join(name)));
        for path in protected_roots(&self.cwd).map_err(Error::state_io_from)? {
            literal_path(&path)?;
            deny.push(path);
        }
        #[cfg(target_os = "macos")]
        for name in PROTECTED {
            deny.extend(
                protected_patterns(name)
                    .into_iter()
                    .map(|pattern| self.cwd.join(pattern)),
            );
        }
        if self.sandbox == Sandbox::ReadOnly {
            deny = vec![PathBuf::from("/")];
        }
        Ok(settings::Filesystem { deny_write: deny })
    }

    pub(crate) fn writable(&self, path: &str) -> bool {
        self.check_writable(path)
            .inspect_err(|error| {
                crate::observability::observation_failed("claude_write_permission", error);
            })
            .unwrap_or(false)
    }
    fn check_writable(&self, path: &str) -> io::Result<bool> {
        if self.sandbox == Sandbox::FullAccess {
            return Ok(true);
        }
        if self.sandbox == Sandbox::ReadOnly || path.starts_with('~') {
            return Ok(false);
        }
        let path = Path::new(path);
        if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Ok(false);
        }
        let cwd = self.cwd.canonicalize()?;
        let protected_roots = protected_roots(&cwd)?;
        let Ok(relative) = path.strip_prefix(&cwd) else {
            return Ok(false);
        };
        if protected_roots.iter().any(|root| path.starts_with(root)) {
            return Ok(false);
        }
        if relative
            .components()
            .any(|part| protected(part.as_os_str()))
        {
            return Ok(false);
        }
        // Each prefix must pass symlink and canonical-root checks before the next.
        let mut walked = PathBuf::new();
        for component in path.components() {
            walked.push(component.as_os_str());
            match fs::symlink_metadata(&walked) {
                Ok(meta) if meta.file_type().is_symlink() => return Ok(false),
                Ok(_) => {
                    let canonical = walked.canonicalize()?;
                    if canonical.file_name().is_some_and(protected)
                        || protected_roots
                            .iter()
                            .any(|root| canonical.starts_with(root))
                    {
                        return Ok(false);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(path != cwd)
    }
}
// Share actual spellings and canonical symlink targets between the native Bash
// sandbox and builtin file-tool policy. Recheck on each tool admission because
// workspace entries may have changed since process launch.
fn protected_roots(cwd: &Path) -> io::Result<Vec<PathBuf>> {
    // Keep directory errors and canonicalization in filesystem observation order.
    let mut roots = Vec::new();
    for entry in fs::read_dir(cwd)? {
        let entry = entry?;
        if protected(&entry.file_name()) {
            roots.push(entry.path());
            roots.push(entry.path().canonicalize()?);
        }
    }
    Ok(roots)
}
fn literal_path(path: &Path) -> Result<()> {
    if path.to_str().is_none() {
        return Err(Error::invalid_path(
            "Claude sandbox paths must be valid UTF-8.",
        ));
    }
    if path
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .any(|c| b"*?[]".contains(c))
    {
        return Err(Error::invalid_path(
            "Claude sandbox paths cannot contain glob metacharacters.",
        ));
    }
    Ok(())
}
fn protected(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        let folded = name.chars().fold(String::new(), |mut folded, c| {
            if let Some((_, ascii)) = ALIASES.iter().find(|(alias, _)| *alias == c) {
                folded.push_str(ascii);
            } else {
                folded.push(c.to_ascii_lowercase());
            }
            folded
        });
        PROTECTED.contains(&folded.as_str())
    })
}
// Unicode 17 default full case folding: these are the only non-ASCII folds
// occurring within this fixed ASCII PROTECTED set. Re-audit when adding names.
// https://www.unicode.org/Public/17.0.0/ucd/CaseFolding.txt
const ALIASES: &[(char, &str)] = &[('ſ', "s"), ('\u{212a}', "k"), ('ﬁ', "fi")];

#[cfg(target_os = "macos")]
fn protected_patterns(name: &str) -> Vec<String> {
    let Some(c) = name.chars().next() else {
        return vec![String::new()];
    };
    let atom = if c.is_ascii_alphabetic() {
        format!("[{}{}]", c, c.to_ascii_uppercase())
    } else {
        c.to_string()
    };
    protected_patterns(&name[c.len_utf8()..])
        .into_iter()
        .map(|suffix| format!("{atom}{suffix}"))
        .chain(ALIASES.iter().flat_map(|(alias, ascii)| {
            name.strip_prefix(ascii).into_iter().flat_map(move |rest| {
                protected_patterns(rest)
                    .into_iter()
                    .map(move |suffix| format!("{alias}{suffix}"))
            })
        }))
        .collect()
}

const PROTECTED: &[&str] = &[
    ".git",
    ".claude",
    ".codex",
    ".mcp.json",
    ".envrc",
    ".husky",
    ".vscode",
    ".idea",
    ".bashrc",
    ".bash_profile",
    ".profile",
    ".zshrc",
    ".zprofile",
    ".zshenv",
];

#[cfg(test)]
mod tests {
    use crate::error::Error;
    #[test]
    fn history_projects_exact_native_arguments() {
        use super::{History, IdentityPrompt};
        let histories = [
            History::New(IdentityPrompt::NativeSnapshot),
            History::Resume(IdentityPrompt::NativeSnapshot),
            History::New(IdentityPrompt::CurrentAgent),
            History::Resume(IdentityPrompt::CurrentAgent),
            History::Fork {
                cutoff: "cutoff".into(),
                source: "source".into(),
            },
        ];
        let expected = [
            vec!["--session-id", "child"],
            vec!["--resume", "child"],
            vec!["--system-prompt-snapshot", "off", "--session-id", "child"],
            vec!["--system-prompt-snapshot", "off", "--resume", "child"],
            vec![
                "--system-prompt-snapshot",
                "off",
                "--resume",
                "source",
                "--fork-session",
                "--session-id",
                "child",
            ],
        ];
        for (history, expected) in histories.into_iter().zip(expected) {
            let mut command = std::process::Command::new("unused");
            history.configure("child", &mut command);
            assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
        }
    }

    #[test]
    fn non_utf8_sandbox_paths_are_rejected_before_serialization() {
        use std::os::unix::ffi::OsStringExt;
        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 0xff]));
        assert!(matches!(
            super::literal_path(&path),
            Err(Error::InvalidPath { .. })
        ));
    }
}

#[derive(serde::Serialize)]
struct SdkServer {
    #[serde(rename = "type")]
    kind: &'static str,
    name: &'static str,
}
#[derive(serde::Serialize)]
struct McpConfiguration {
    #[serde(rename = "mcpServers")]
    servers: Value,
}
