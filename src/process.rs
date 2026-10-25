//! Owned native-process launch and shared exit observation.
// POSIX shell convention: the requested command could not execute.
use crate::{
    config::Provider,
    error::{Error, Result},
    launch::{self, LauncherCache},
    os::{self, Signal},
    state::{ChildReceipt, Operation},
};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io::{Read, Write},
    os::{
        fd::BorrowedFd,
        unix::io::{AsFd, AsRawFd},
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
pub(crate) const EXEC_FAILED_EXIT: i32 = 126;
// Bound final process-group disappearance checks after the leader is reaped.
const GROUP_EXIT_GRACE: Duration = Duration::from_secs(2);

mod cancel;
mod captured;
mod environment;
mod login;
mod owned;
mod version;

pub(crate) use cancel::Cancellation;
pub(crate) use captured::{Captured, CapturedOutput, malformed};
#[cfg(test)]
pub(crate) use environment::{EnvironmentVariable, child_environment};
pub(crate) use environment::{command, executable, runtime_codex_executable};
pub(crate) use login::{LoginExit, interactive};
pub(crate) use owned::{CleanupReceipt, OwnedGroup, wait_group_absent, wait_until};
pub(crate) use version::version_probe;

// Interval between captured-helper response and exit checks.
const CAPTURE_POLL_INTERVAL: Duration = Duration::from_millis(10);
// Give the helper time to flush provider-owned files after EOF before forced cleanup.
const CAPTURE_SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
// Give interactive login time to handle SIGINT before escalating to SIGTERM.
const LOGIN_INTERRUPT_GRACE: Duration = Duration::from_secs(2);
// Give interactive login time to handle SIGTERM before forced cleanup.
const LOGIN_TERMINATION_GRACE: Duration = Duration::from_secs(5);
// Interval between interactive-login cancellation and exit checks.
const LOGIN_POLL_INTERVAL: Duration = Duration::from_millis(20);
// Bound each capture/terminal I/O syscall without starving control work.
const CAPTURE_READ_BYTES: usize = 8192;
// A second cancellation bypasses the login SIGINT grace.
const LOGIN_SINGLE_INTERRUPT: usize = 1;

// Maximum combined stdout and stderr bytes captured from a provider helper.
pub(crate) const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;

struct Spawned {
    child: Child,
    receipt: ChildReceipt,
    terminal: Option<os::terminal::Terminal>,
}
fn spawn(
    launcher: &LauncherCache,
    command: &mut Command,
    operation: &mut Operation,
    group: bool,
    owner: Option<BorrowedFd<'_>>,
) -> Result<Spawned> {
    operation.configure(command);
    let receipt = operation.prepare_child(true, !group)?;
    let terminal = (!group)
        .then(os::terminal::Terminal::open)
        .transpose()
        .map_err(Error::state_io_from)?;
    let mut inherited = vec![receipt.fd().as_raw_fd()];
    inherited.extend(owner.map(|fd| fd.as_raw_fd()));
    let outcome = launch::spawn_with_terminal(
        launcher,
        command,
        group,
        &inherited,
        terminal.as_ref().map(|t| &t.slave),
        |pid| {
            receipt.started(pid)?;
            Ok(())
        },
    );
    let child = match outcome {
        Ok(child) => child,
        Err(error) => {
            let cleanup = if matches!(error, Error::SpawnFailed { .. }) {
                receipt.not_started()
            } else {
                receipt.finish(false)
            };
            if let Err(secondary) = cleanup {
                tracing::warn!(name: "process.spawn_cleanup_failed",
                     code = error.code(),
                     primary = &error as &(dyn std::error::Error + 'static),
                     secondary = &secondary as &(dyn std::error::Error + 'static),
                     decision = "preserve_spawn_error");
            }
            return Err(error);
        }
    };
    Ok(Spawned {
        child,
        receipt,
        terminal,
    })
}

pub(crate) fn has_exited(child: &Child) -> Result<bool> {
    os::has_exited(child).map_err(|cause| {
        Error::process_wait_failed("Cannot confirm provider process termination.")
            .with_source(cause)
    })
}

#[cfg(test)]
mod tests;
