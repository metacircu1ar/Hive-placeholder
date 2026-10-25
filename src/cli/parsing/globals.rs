//! Explicit globals must have a consumer; defaults do not assert user intent.
use super::{Exit, conditional};
use clap::{ArgMatches, parser::ValueSource};
use hive_input::Code;

pub(super) const GLOBALS: &[(&str, &str)] = &[
    ("config", "--config"),
    ("state_dir", "--state-dir"),
    ("codex_bin", "--codex-bin"),
    ("claude_bin", "--claude-bin"),
    ("probe_timeout", "--probe-timeout"),
    ("login_timeout", "--login-timeout"),
    ("managed_service", "--managed-service"),
];

pub(super) fn explicit(matches: &ArgMatches) -> Vec<&'static str> {
    let mut selected = matches;
    while let Some((_, child)) = selected.subcommand() {
        selected = child;
    }
    GLOBALS
        .iter()
        .filter(|(id, _)| selected.value_source(id) == Some(ValueSource::CommandLine))
        .map(|(_, flag)| *flag)
        .collect()
}

pub(in crate::cli) fn removal(flags: &[&'static str], purge: bool) -> crate::error::Result<()> {
    for &flag in flags {
        let expected = match flag {
            "--config" | "--state-dir" if purge => continue,
            "--config" | "--state-dir" => "use --uninstall --purge, or omit this scope option",
            "--codex-bin" | "--claude-bin" | "--probe-timeout" | "--login-timeout"
            | "--managed-service" => "omit this setup option with --uninstall",
            _ => {
                return Err(crate::error::Error::internal_error(
                    "Unclassified setup global.",
                ));
            }
        };
        return Err(crate::cli::input::Context::new("setup", flag)
            .invalid(
                Code::ConflictingArguments,
                "Option is not used by this removal mode.",
                expected,
            )
            .error);
    }
    Ok(())
}

/// Shared with generated CLI suggestions so their prefixes obey the same rules.
pub(in crate::cli) fn consumes(path: &str, argument: &str) -> bool {
    let auth_probe = matches!(
        path,
        "auth status" | "auth login" | "auth recover" | "auth repair"
    );
    match argument {
        "config" => {
            auth_probe
                || matches!(
                    path,
                    "setup"
                        | "setup-maintenance"
                        | "doctor"
                        | "config check"
                        | "models"
                        | "verify-steering"
                        | "auth list"
                        | "daemon run"
                        | "daemon start"
                        | "daemon restart"
                        | "daemon stop"
                        | "daemon status"
                        | "service install"
                        | "service start"
                        | "service stop"
                        | "service status"
                        | "service uninstall"
                        | "tui"
                )
        }
        "state_dir" => !matches!(path, "version" | "config check"),
        "codex_bin" | "claude_bin" => {
            auth_probe
                || matches!(
                    path,
                    "setup"
                        | "doctor"
                        | "models"
                        | "daemon run"
                        | "daemon start"
                        | "daemon restart"
                        | "service install"
                )
                || (argument == "claude_bin" && matches!(path, "verify-steering"))
        }
        "probe_timeout" => auth_probe || matches!(path, "setup" | "doctor"),
        "login_timeout" => matches!(path, "setup" | "auth login" | "auth repair"),
        "managed_service" => path == "daemon run",
        _ => false,
    }
}

pub(super) fn validate(matches: &ArgMatches, daemon: bool) -> Result<(), Exit> {
    let mut selected = matches;
    let mut path = Vec::new();
    while let Some((name, child)) = selected.subcommand() {
        path.push(name);
        selected = child;
    }
    let path = path.join(" ");
    if path == "collectors status"
        && selected.value_source("request_id") == Some(ValueSource::CommandLine)
    {
        return Err(conditional(
            &path,
            Code::ConflictingArguments,
            "--request-id",
            "omit this option: collector status is read-only",
        ));
    }
    for &(argument, flag) in GLOBALS {
        if selected.value_source(argument) == Some(ValueSource::CommandLine)
            && !consumes(&path, argument)
        {
            return Err(conditional(
                if daemon {
                    "hive-daemon"
                } else if path == "setup-maintenance" {
                    "setup"
                } else {
                    &path
                },
                Code::ConflictingArguments,
                flag,
                "omit this option: the selected command does not use it",
            ));
        }
    }
    Ok(())
}

/// Presence matters even when an explicit value equals the parser default.
pub(super) fn explicit_provider(matches: &ArgMatches) -> Option<&'static str> {
    let mut selected = matches;
    while let Some((_, child)) = selected.subcommand() {
        selected = child;
    }
    [("codex_bin", "--codex-bin"), ("claude_bin", "--claude-bin")]
        .into_iter()
        .find_map(|(id, flag)| {
            (selected.value_source(id) == Some(ValueSource::CommandLine)).then_some(flag)
        })
}
