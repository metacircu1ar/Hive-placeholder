//! Interactive native login terminal relay and shutdown.
// Bound output work per relay tick so continuous output cannot starve controls.
use super::{
    CAPTURE_POLL_INTERVAL, Cancellation, CleanupReceipt, Command, Duration, Error, ExitStatus,
    Instant, LOGIN_INTERRUPT_GRACE, LOGIN_POLL_INTERVAL, LOGIN_SINGLE_INTERRUPT,
    LOGIN_TERMINATION_GRACE, LauncherCache, Operation, Ordering, OwnedGroup, Read, Result, Signal,
    Spawned, Stdio, Write, has_exited, os, spawn, wait_until,
};
use std::{
    fs,
    io::{self, ErrorKind},
    thread,
};
const TERMINAL_READ_BATCH: usize = 8;
const TERMINAL_OUTPUT_BYTES: usize = TERMINAL_READ_BATCH * super::CAPTURE_READ_BYTES;
// Allow the final terminal page to drain without blocking login completion indefinitely.
const TERMINAL_DRAIN_GRACE: Duration = Duration::from_secs(2);

pub(crate) struct LoginExit {
    pub(crate) status: ExitStatus,
    pub(crate) timed_out: bool,
}

struct LoginTerminal {
    terminal: os::terminal::Terminal,
    pending: Vec<u8>,
    output: Vec<u8>,
    eof: bool,
}
impl LoginTerminal {
    fn relay_output(&mut self) -> Result<bool> {
        let mut bytes = [0; super::CAPTURE_READ_BYTES];
        let mut read_any = false;
        // Bound output work so continuous native output cannot starve controls.
        for _ in 0..TERMINAL_READ_BATCH {
            if self.output.len() >= TERMINAL_OUTPUT_BYTES {
                break;
            }
            match self.terminal.master.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => {
                    read_any = true;
                    self.output.extend_from_slice(&bytes[..n]);
                }
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {
                    break;
                }
                Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
                Err(e) => return Err(Error::state_io_from(e)),
            }
        }
        if self.eof {
            self.output.clear();
        } else if let Err(error) = flush_terminal(&mut self.terminal.output, &mut self.output) {
            if terminal_hangup(&error) {
                self.eof = true;
                self.output.clear();
            } else {
                return Err(error);
            }
        }
        Ok(read_any || !self.output.is_empty())
    }
    fn relay(&mut self) -> Result<()> {
        self.relay_output()?;
        let mut bytes = [0; super::CAPTURE_READ_BYTES];
        if !self.eof
            && self.pending.len() < super::CAPTURE_READ_BYTES
            && self.terminal.input_ready().map_err(Error::state_io_from)?
        {
            let room = super::CAPTURE_READ_BYTES - self.pending.len();
            match self.terminal.input.read(&mut bytes[..room]) {
                Ok(0) => self.eof = true,
                Ok(n) => self.pending.extend_from_slice(&bytes[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) => {}
                Err(e) if matches!(e.raw_os_error(), Some(libc::EIO | libc::ENXIO)) => {
                    self.eof = true;
                }
                Err(e) => return Err(Error::state_io_from(e)),
            }
        }
        if let Err(error) = flush_terminal(&mut self.terminal.master, &mut self.pending) {
            if terminal_hangup(&error) {
                self.pending.clear();
            } else {
                return Err(error);
            }
        }
        Ok(())
    }
    fn drain_output(&mut self) -> Result<()> {
        let deadline = Instant::now() + TERMINAL_DRAIN_GRACE;
        if wait_until(deadline, CAPTURE_POLL_INTERVAL, || {
            self.relay_output().map(|busy| !busy)
        })? {
            Ok(())
        } else {
            Err(Error::provider_io(
                "Login terminal output remained blocked.",
            ))
        }
    }
}

fn flush_terminal(stream: &mut fs::File, pending: &mut Vec<u8>) -> Result<()> {
    while !pending.is_empty() {
        match stream.write(pending) {
            Ok(0) => {
                return Err(Error::provider_io("Login terminal closed.")
                    .with_source(io::Error::from_raw_os_error(libc::EPIPE)));
            }
            Ok(n) => {
                pending.drain(..n);
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {
                break;
            }
            Err(e) => return Err(Error::state_io_from(e)),
        }
    }
    Ok(())
}
fn terminal_hangup(error: &Error) -> bool {
    // Native errno distinctions are finer than std::io::ErrorKind.
    matches!(
        error.io_raw_os_error(),
        Some(libc::EIO | libc::ENXIO | libc::EPIPE)
    )
}

#[derive(Clone, Copy)]
enum LoginStop {
    Cancelled,
    TimedOut,
}
enum LoginEscalation {
    Running,
    Interrupted { at: Instant, reason: LoginStop },
    Terminating { at: Instant, reason: LoginStop },
}
impl LoginEscalation {
    fn timed_out(&self) -> bool {
        matches!(
            self,
            Self::Interrupted {
                reason: LoginStop::TimedOut,
                ..
            } | Self::Terminating {
                reason: LoginStop::TimedOut,
                ..
            }
        )
    }
}

#[expect(
    clippy::expect_used,
    reason = "spawn with group=false creates a controlling terminal before returning \
        successfully."
)]
pub(crate) fn interactive(
    launcher: &LauncherCache,
    command: &mut Command,
    operation: &mut Operation,
    timeout: Duration,
    cancellation: &Cancellation,
) -> Result<LoginExit> {
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    // Own a dedicated foreground group so native codes/raw mode still work.
    let Spawned {
        child,
        receipt,
        terminal,
    } = spawn(launcher, command, operation, false, None)?;
    let mut terminal = LoginTerminal {
        terminal: terminal.expect("interactive terminal"),
        pending: Vec::new(),
        output: Vec::new(),
        eof: false,
    };
    let mut owned = OwnedGroup::new(child, receipt, CleanupReceipt::Unconfirmed);
    let deadline = Instant::now() + timeout;
    let mut escalation = LoginEscalation::Running;
    let mut hung_up = false;
    loop {
        terminal.relay()?;
        if terminal.eof && !hung_up {
            if cancellation.count() == 0 {
                let _ = cancellation.signals.compare_exchange(
                    cancellation.checkpoint,
                    cancellation.checkpoint + 1,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
            }
            hung_up = true;
        }
        if has_exited(&owned.child)? {
            return finish_interactive(&mut owned, &mut terminal, &escalation);
        }
        escalation.advance(&mut owned, cancellation, deadline)?;
        thread::sleep(LOGIN_POLL_INTERVAL);
    }
}

fn finish_interactive(
    owned: &mut OwnedGroup,
    terminal: &mut LoginTerminal,
    escalation: &LoginEscalation,
) -> Result<LoginExit> {
    let clean = os::shell::group_only_leader(owned.child.id()).unwrap_or_else(|error| {
        tracing::warn!(name: "login.cleanup_probe_failed",
             pid = owned.child.id(),
             error = &error as &(dyn std::error::Error + 'static),
             decision = "retain_evidence");
        false
    });
    if !clean {
        owned.signal(Signal::KILL);
    }
    let status = owned.child.wait().map_err(|cause| {
        Error::process_wait_failed("Cannot reap the login helper.").with_source(cause)
    })?;
    owned.reaped = true;
    let deadline = Instant::now() + super::GROUP_EXIT_GRACE;
    wait_until(deadline, CAPTURE_POLL_INTERVAL, || {
        os::group_absent(owned.child.id()).map_err(Error::state_io_from)
    })?;
    owned
        .receipt
        .finish(clean && os::group_absent(owned.child.id()).map_err(Error::state_io_from)?)?;
    // Display failure cannot suppress account verification after reap.
    if let Err(error) = terminal.drain_output() {
        tracing::warn!(name: "login.terminal_drain_failed",
             error = &error as &(dyn std::error::Error + 'static),
             decision = "continue_verification");
    }
    Ok(LoginExit {
        status,
        timed_out: escalation.timed_out(),
    })
}

impl LoginEscalation {
    fn advance(
        &mut self,
        owned: &mut OwnedGroup,
        cancellation: &Cancellation,
        deadline: Instant,
    ) -> Result<()> {
        let cancelling = cancellation.count() > 0 || Instant::now() >= deadline;
        if cancelling {
            if matches!(*self, LoginEscalation::Running) {
                let reason = if cancellation.count() == 0 {
                    LoginStop::TimedOut
                } else {
                    LoginStop::Cancelled
                };
                owned.signal(Signal::INT);
                *self = LoginEscalation::Interrupted {
                    at: Instant::now(),
                    reason,
                };
            }
            if let LoginEscalation::Interrupted { at, reason } = *self
                && (cancellation.count() > LOGIN_SINGLE_INTERRUPT
                    || at.elapsed() >= LOGIN_INTERRUPT_GRACE)
            {
                owned.signal(Signal::TERM);
                *self = LoginEscalation::Terminating {
                    at: Instant::now(),
                    reason,
                };
            }
            if matches!(
                *self,
                LoginEscalation::Terminating { at, .. }
                    if at.elapsed() >= LOGIN_TERMINATION_GRACE
            ) {
                owned.signal(Signal::KILL);
                match owned.child.wait() {
                    Ok(_) => owned.reaped = true,
                    Err(error) => {
                        tracing::warn!(name: "login.cleanup_wait_failed",
                             pid = owned.child.id(),
                             error = &error as &(dyn std::error::Error + 'static),
                             decision = "retain_evidence");
                    }
                }
                // Keep the writer receipt: forced termination cannot verify provider writes.
                return Err(Error::recovery_required(concat!(
                    "Login required forced termination. Retry auth recover once its process ",
                    "group is gone; isolated auth repair --reauthenticate remains available. ",
                    "Existing credentials and execution evidence are preserved.",
                )));
            }
        }
        Ok(())
    }
}
