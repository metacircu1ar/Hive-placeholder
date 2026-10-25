//! Retain native group ownership until reap and finish the caller's cleanup evidence.
use super::{CAPTURE_POLL_INTERVAL, Child, Duration, Instant, Result, Signal, os};
use crate::{error::Error, state::ChildReceipt};
use std::{result, thread};

pub(crate) enum CleanupReceipt {
    GroupAbsence,
    Unconfirmed,
}

pub(crate) struct OwnedGroup {
    pub(crate) child: Child,
    pub(crate) receipt: ChildReceipt,
    pub(crate) reaped: bool,
    cleanup: CleanupReceipt,
}
impl OwnedGroup {
    pub(crate) fn new(child: Child, receipt: ChildReceipt, cleanup: CleanupReceipt) -> Self {
        Self {
            child,
            receipt,
            reaped: false,
            cleanup,
        }
    }
    /// Cleanup continues after a failed signal; retain the error before reap.
    pub(crate) fn signal(&self, signal: Signal) {
        if let Err(error) = os::signal_group(&self.child, signal) {
            if os::no_such_process(&error) {
                tracing::debug!(name: "process.cleanup_already_exited",
                     pid = self.child.id(),
                     error = &error as &(dyn std::error::Error + 'static),
                     decision = "continue_reap");
            } else {
                tracing::warn!(name: "process.cleanup_signal_failed",
                     pid = self.child.id(),
                     error = &error as &(dyn std::error::Error + 'static),
                     decision = "continue_reap");
            }
        }
    }
}
impl Drop for OwnedGroup {
    fn drop(&mut self) {
        if !self.reaped {
            self.signal(Signal::KILL);
            if let Err(error) = self.child.wait() {
                tracing::warn!(name: "process.cleanup_wait_failed",
                     pid = self.child.id(),
                     error = &error as &(dyn std::error::Error + 'static),
                     decision = "check_group_absence");
            }
            // Captured probes may confirm group absence; an interrupted login
            // cannot prove that the provider finished its credential writes.
            let confirmed = match self.cleanup {
                CleanupReceipt::GroupAbsence => match wait_group_absent(self.child.id()) {
                    Ok(absent) => absent,
                    Err(error) => {
                        tracing::warn!(name: "process.cleanup_probe_failed",
                             pid = self.child.id(),
                             code = error.code(),
                             error = &error as &(dyn std::error::Error + 'static),
                             decision = "retain_evidence");
                        false
                    }
                },
                CleanupReceipt::Unconfirmed => false,
            };
            if let Err(error) = self.receipt.finish(confirmed) {
                tracing::warn!(name: "process.cleanup_receipt_failed",
                     pid = self.child.id(),
                     code = error.code(),
                     error = &error as &(dyn std::error::Error + 'static),
                     decision = "retain_evidence");
            }
        }
    }
}

pub(crate) fn wait_until<E>(
    deadline: Instant,
    interval: Duration,
    mut ready: impl FnMut() -> result::Result<bool, E>,
) -> result::Result<bool, E> {
    loop {
        if ready()? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(interval);
    }
}

pub(crate) fn wait_group_absent(pid: u32) -> Result<bool> {
    let deadline = Instant::now() + super::GROUP_EXIT_GRACE;
    wait_until(deadline, CAPTURE_POLL_INTERVAL, || {
        // An adopted member that exited stays in the group until reaped.
        os::reap_group(pid).map_err(Error::state_io_from)?;
        os::group_absent(pid).map_err(Error::state_io_from)
    })
}

#[cfg(test)]
mod tests;
