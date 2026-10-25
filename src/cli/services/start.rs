//! Keep startup input checks inside the service controller's scope lock.
use super::super::{Cli, CommandOutcome, Path, Result, daemon::selection::Expected, input};
use std::{
    cell::RefCell,
    time::{Duration, Instant},
};

pub(super) fn overrides(cli: &Cli) -> Result<()> {
    if let Some(flag) = cli.explicit_provider {
        return Err(input::Context::new("daemon start", flag)
            .invalid(
                hive_input::Code::ConflictingArguments,
                "A managed daemon uses its installed provider selection.",
                "omit this option; change provider paths with service install before starting",
            )
            .error);
    }
    Ok(())
}

/// Controller health callbacks return `io::Error`. Capture the original typed
/// error or wire reply so this adapter never strips input metadata/native causes.
pub(super) fn checked(
    expected: &Expected,
    root: &Path,
    start: impl FnOnce(&dyn Fn() -> std::io::Result<bool>) -> std::io::Result<()>,
) -> Result<Option<CommandOutcome>> {
    let failure = RefCell::new(None);
    let health = || {
        let observation = expected.observe(root, Instant::now() + HEALTH_TIMEOUT);
        match observation {
            Ok(None) => Ok(false),
            Ok(Some(outcome)) if outcome.success => Ok(true),
            result => {
                *failure.borrow_mut() = Some(result);
                Err(std::io::Error::other("Daemon startup observation failed."))
            }
        }
    };
    let result = start(&health);
    if let Some(original) = failure.into_inner() {
        return original;
    }
    result.map_err(super::error)?;
    Ok(None)
}

pub(super) fn expectation(config: &Path, label: String) -> Result<Expected> {
    Expected::load(config, None, Some(label))
}

pub(super) fn run(
    cli: &Cli,
    config: &Path,
    root: &Path,
    label: &str,
    controller: &hive_setup_support::service::Controller<'_>,
    health: &dyn Fn() -> std::io::Result<bool>,
) -> Result<Option<CommandOutcome>> {
    if matches!(
        cli.command,
        super::super::Command::Daemon {
            command: super::super::DaemonCommand::Start
        }
    ) {
        overrides(cli)?;
        let spec = hive_setup_support::service::load(controller.layout, controller.scope)
            .map_err(super::error)?
            .ok_or_else(|| {
                super::error(hive_setup_support::invalid("Service is not installed."))
            })?;
        let expected = expectation(config, label.to_owned())?.program(&spec.program)?;
        checked(&expected, root, |probe| controller.start(probe))
    } else {
        controller.start(health).map_err(super::error)?;
        Ok(None)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;

const HEALTH_TIMEOUT: std::time::Duration = Duration::from_secs(2);
