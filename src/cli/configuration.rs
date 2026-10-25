//! Local configuration checks; fixes use setup's locked, backed-up publisher.
use super::{CommandOutcome, Config, Result};
use crate::config::deprecations;
use crate::runtime::api;
use clap::Subcommand;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub(super) enum Command {
    /// Validate configuration and report deprecated keys without provider calls.
    Check {
        /// Apply known safe key renames with a private backup; no daemon restart.
        #[arg(long)]
        fix: bool,
    },
}
pub(super) fn run(path: &Path, command: &Command) -> Result<CommandOutcome> {
    match command {
        Command::Check { fix: false } => {
            let config = Config::load(path)?;
            Ok(CommandOutcome::ok(api::value(Configuration {
                valid: true,
                config: path,
                changed: false,
                applied: Vec::<String>::new(),
                deprecations: &config.deprecations,
                backup: None,
                auth_groups: config.auth_group_summary(),
            })))
        }
        Command::Check { fix: true } => fix(path, deprecations::TABLE),
    }
}
fn fix(path: &Path, table: &[deprecations::Entry]) -> Result<CommandOutcome> {
    let snapshot = super::setup::publication::Snapshot::inspect(path)?;
    let correction = deprecations::correct(&snapshot.text, table, snapshot.target())?;
    let groups = Config::parse_at(&correction.text, snapshot.target())?
        .auth_group_summary()
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect::<std::collections::BTreeMap<_, _>>();
    let changed = correction.text != snapshot.text;
    // Even a no-op must preserve bytes/comments, metadata, and absence of backups.
    let backup = if changed {
        snapshot.publish(&correction.text)?
    } else {
        None
    };
    deprecations::warn(&correction.remaining);
    Ok(CommandOutcome::ok(api::value(Configuration {
        valid: true,
        config: path,
        changed,
        applied: correction.applied,
        deprecations: correction.remaining,
        backup,
        auth_groups: groups,
    })))
}

#[derive(Serialize)]
struct Configuration<'a, A, D, G> {
    valid: bool,
    config: &'a Path,
    changed: bool,
    applied: A,
    deprecations: D,
    backup: Option<PathBuf>,
    auth_groups: G,
}

#[cfg(test)]
mod tests;
