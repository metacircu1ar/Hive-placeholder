//! Capture native version output while callers retain their validation and error precedence.
use super::{
    Cancellation, Captured, CapturedOutput, Command, Duration, LauncherCache, Operation, Result,
};
use std::os::fd::BorrowedFd;

pub(crate) fn version_probe<T>(
    launcher: &LauncherCache,
    command: &mut Command,
    operation: &mut Operation,
    timeout: Duration,
    cancellation: Cancellation,
    owner: Option<BorrowedFd<'_>>,
    inspect: impl FnOnce(CapturedOutput, &Operation) -> Result<T>,
) -> Result<T> {
    let mut child =
        Captured::start_with_owner(launcher, command, operation, timeout, cancellation, owner)?;
    let output = child.output()?;
    // Keep the captured pipes and receipt alive through caller validation.
    inspect(output, operation)
}

#[cfg(test)]
mod tests;
