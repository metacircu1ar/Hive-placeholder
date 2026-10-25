//! Protocol quiescence and process shutdown while native ownership remains held.
//! Drain loops keep I/O, lifecycle observations and cleanup proof in order.
use super::super::{limits::NATIVE_EOF_GRACE, protocol::outgoing};
use super::{IO_POLL_INTERVAL, Rpc, Shutdown, Turn};
use crate::runtime::provider::ShutdownMethod;
use crate::runtime::provider::error::CLEANING_UP;
use crate::{
    error::{Error, Result},
    os::{self, Signal},
    process,
};
use std::process::ExitStatus;
use std::thread;
use std::time::{Duration, Instant};

// Wait for turn completion before classifying provider cleanup as uncertain.
const INTERRUPT_GRACE: Duration = Duration::from_secs(10);
// Wait for background terminals to disappear before classifying cleanup as uncertain.
const TERMINAL_CLEANUP_GRACE: Duration = Duration::from_secs(10);
// Interval between background-terminal list requests during cleanup.
const TERMINAL_POLL_INTERVAL: Duration = Duration::from_millis(100);
// Allow the native shutdown handler to finish after the launcher receives SIGTERM.
const SIGTERM_EXIT_GRACE: Duration = Duration::from_secs(10);
// Limit final frames inspected after exit before classifying cleanup as uncertain.
const EXIT_FRAME_DRAIN_BUDGET: i32 = 64;
// Native request rejection or terminal failure can exit with status 1 after proven cleanup.
const NATIVE_FAILURE_EXIT_CODE: i32 = 1;

struct ExitAttempt {
    method: crate::runtime::provider::ShutdownMethod,
    exited: Result<bool>,
}
struct CleanupEvidence {
    quiescent: bool,
    preinput: bool,
}

impl Rpc {
    fn cleanup_error(&self, phase: &'static str, error: &(dyn std::error::Error + 'static)) {
        tracing::warn!(name: "provider.cleanup_failed",
             pid = self.pipes.child.id(),
             phase = phase,
             error = error,
             decision = "retain_evidence");
    }
    fn abort_claude_turn(&self) -> bool {
        self.claude.is_some()
            && self.protocol_uncertain
            && self.threads.values().any(|state| {
                state
                    .turn
                    .as_ref()
                    .is_some_and(|turn| !matches!(turn, Turn::Idle))
            })
    }
    fn quiesce(&mut self) -> Result<bool> {
        // An unfinished observation never certifies shutdown/recovery. Close
        // all exemptions and retain uncertainty before the strict cleanup path.
        let draining: Vec<_> = self
            .threads
            .iter()
            .filter(|(_, state)| state.drain.is_some())
            .map(|(thread, _)| thread.clone())
            .collect();
        for thread in draining {
            self.quarantine(&thread);
        }
        for state in self.threads.values_mut() {
            state.drain = None;
        }
        if self.abort_claude_turn() {
            return Ok(false);
        }
        let mut clean = self.claude.is_some() || !self.unresolved_lifecycle();
        let threads: Vec<_> = self
            .threads
            .iter()
            .filter(|(_, state)| state.turn.is_some())
            .map(|(thread, _)| thread.clone())
            .collect();
        for thread in threads {
            // Attempt every identifiable conversation even if a sibling cannot
            // be confirmed. No failed cleanup evidence is cleared by later work.
            let quiescent = self.quiesce_thread(&thread).unwrap_or_else(|error| {
                self.cleanup_error("quiesce_thread", &error);
                false
            });
            if !quiescent {
                clean = false;
            }
        }
        clean &= self.quiesce_claude()?;
        Ok(clean
            && !self.unresolved_lifecycle()
            && !self.protocol_uncertain
            && !self.has_quarantined)
    }
    fn quiesce_claude(&mut self) -> Result<bool> {
        let mut clean = true;
        if let Some(codec) = &mut self.claude {
            codec.guard.seal();
            let deadline = Instant::now() + TERMINAL_CLEANUP_GRACE;
            while !self
                .claude
                .as_ref()
                .is_some_and(super::super::claude::Codec::quiescent)
                && Instant::now() < deadline
            {
                if self.abort_claude_turn() {
                    return Ok(false);
                }
                if let Some(value) = self.next()? {
                    if value.is_response()
                        && value.id().is_some_and(|id| self.requests.contains_key(&id))
                    {
                        // Claude quiescence intentionally ignores unknown responses.
                        self.reconcile_response(&value)?;
                    } else if value.is_request() {
                        self.send(outgoing::unsupported(value.raw_id(), CLEANING_UP))?;
                    }
                } else {
                    self.wait(None, deadline.saturating_duration_since(Instant::now()))?;
                }
            }
            clean &= self
                .claude
                .as_ref()
                .is_some_and(super::super::claude::Codec::quiescent);
        }
        Ok(clean)
    }
    fn quiesce_thread(&mut self, thread: &str) -> Result<bool> {
        if matches!(self.turn(thread), Some(Turn::Submitting)) {
            return Ok(false);
        }
        if let Some(Turn::Active(turn)) = self.turn(thread).cloned() {
            self.interrupt(thread, &turn)?;
            let deadline = Instant::now() + INTERRUPT_GRACE;
            while !matches!(self.turn(thread), Some(Turn::Idle)) && Instant::now() < deadline {
                if let Some(value) = self.next()? {
                    if value.is_response() {
                        self.reconcile_response(&value)?;
                    } else if value.is_request() {
                        self.send(outgoing::unsupported(value.raw_id(), CLEANING_UP))?;
                    }
                } else {
                    self.wait(None, deadline.saturating_duration_since(Instant::now()))?;
                }
            }
            if !matches!(self.turn(thread), Some(Turn::Idle)) {
                return Ok(false);
            }
        }
        if self.claude.is_some() {
            // Pending control receipts and shell completion are drained once,
            // after the shared turn lifecycle settles and admission is sealed.
            return Ok(true);
        }
        self.call(outgoing::terminal_clean(thread))?;
        let deadline = Instant::now() + TERMINAL_CLEANUP_GRACE;
        loop {
            let listed = self.call_with_timeout(
                outgoing::terminal_list(thread),
                deadline.saturating_duration_since(Instant::now()),
            )?;
            if listed.terminals_empty()? {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            thread::sleep(TERMINAL_POLL_INTERVAL);
        }
    }
    fn drain_exit_frame(&mut self) -> Result<bool> {
        let (value, observed) = match self.pending.pop_front() {
            Some(value) => (Some(value), true),
            None => (self.read_next()?, false),
        };
        let Some(value) = value else {
            return Ok(false);
        };
        if value.is_response() {
            if let Err(error) = self.reconcile_response(&value) {
                self.cleanup_error("reconcile_exit_response", &error);
            }
        } else if !observed {
            self.observe(&value);
        }
        Ok(true)
    }
    fn wait_for_exit(&mut self, duration: Duration) -> Result<bool> {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            let exited = process::has_exited(&self.pipes.child)?;
            // EOF can race a natural process exit. Other protocol errors still
            // make cleanup uncertain even after terminal cleanup was acknowledged.
            match self.drain_exit_frame() {
                Ok(true) => {
                    continue; // Inspect already-buffered frames even after leader exit.
                }
                Err(error) if !matches!(error, Error::ProviderExited { .. }) => {
                    self.cleanup_error("drain_exit", &error);
                    self.protocol_uncertain = true;
                }
                _ => {}
            }
            if exited {
                return Ok(true);
            }
            thread::sleep(IO_POLL_INTERVAL);
        }
        let exited = process::has_exited(&self.pipes.child)?;
        if exited {
            // A natural exit can race the last sleep interval. Give its final
            // output one bounded drain before classifying cleanup as uncertain.
            for _ in 0..EXIT_FRAME_DRAIN_BUDGET {
                match self.drain_exit_frame() {
                    Ok(true) => {}
                    Ok(false) | Err(Error::ProviderExited { .. }) => return Ok(true),
                    Err(error) => {
                        self.cleanup_error("drain_final_exit", &error);
                        self.protocol_uncertain = true;
                        return Ok(true);
                    }
                }
            }
            self.protocol_uncertain = true;
        }
        Ok(exited)
    }
    fn stop_after_eof(&mut self) -> ExitAttempt {
        let mut method = ShutdownMethod::Eof;
        let mut exited = if self.abort_claude_turn() {
            Ok(false) // Uncertain transport cannot carry a reliable interrupt.
        } else {
            self.wait_for_exit(NATIVE_EOF_GRACE)
        };
        if matches!(exited, Ok(false)) {
            // The Node launcher forwards SIGTERM to the native app-server. Give its
            // graceful shutdown handler a chance to close sessions and tool children.
            method = ShutdownMethod::Sigterm;
            let signalled = os::signal_child(&self.pipes.child, Signal::TERM);
            exited = match signalled {
                Ok(()) => self.wait_for_exit(SIGTERM_EXIT_GRACE),
                Err(error) => {
                    self.cleanup_error("signal_term", &error);
                    Ok(false)
                }
            };
        }
        ExitAttempt { method, exited }
    }
    fn final_cleanup_evidence(&self, quiescent: bool) -> CleanupEvidence {
        // Before any input, initialization metadata failure cannot leave task
        // work behind. This live proof is never reconstructed for an old record.
        // Final draining may observe execution evidence and veto the exception.
        let drained = self.claude.is_none()
            || (self.pipes.stdout_eof
                && self.pipes.stderr_eof
                && self.pipes.buffer.is_empty()
                && self.pending.is_empty()
                && self.claude.as_ref().is_some_and(|codec| !codec.buffered()));
        let preinput = drained
            && self
                .claude
                .as_ref()
                .is_some_and(super::super::claude::Codec::preinput_quiescent);
        // Every Claude pre-input exit uses the complete live proof, even if an
        // earlier ordinary quiescence snapshot succeeded before the final drain.
        let quiescent = quiescent
            && !self.claude_before_input()
            && drained
            && !self.protocol_uncertain
            && !self.has_quarantined;
        CleanupEvidence {
            quiescent,
            preinput,
        }
    }
    fn reap_owned(&mut self, exited: &Result<bool>, confirmed: &mut bool) -> Option<ExitStatus> {
        // Leader is still unreaped, preventing PID/PGID reuse during group cleanup.
        // On wait errors ownership cannot be established; never signal a stale PID.
        let mut can_reap = matches!(exited, Ok(true));
        // A protocol-clean graceful exit needs no signal. On macOS, signalling
        // a group containing only the unreaped zombie can itself return EPERM.
        if !*confirmed && exited.is_ok() {
            let killed = os::signal_group(&self.pipes.child, Signal::KILL);
            can_reap |= killed.is_ok();
            if let Err(error) = &killed {
                if os::no_such_process(error) {
                    tracing::debug!(name: "provider.cleanup_already_exited",
                         pid = self.pipes.child.id(),
                         error = error as &(dyn std::error::Error + 'static),
                         decision = "continue_reap");
                } else {
                    self.cleanup_error("signal_kill", error);
                    *confirmed = false;
                }
            }
        }
        if can_reap {
            // A failed wait supplies no exit proof; close retains the reservation without it.
            match self.pipes.child.wait() {
                Ok(status) => Some(status),
                Err(error) => {
                    self.cleanup_error("reap", &error);
                    None
                }
            }
        } else {
            None
        }
    }
    pub(crate) fn close(&mut self) -> Shutdown {
        if let Some(report) = &self.closed {
            return report.clone();
        }
        if let Some(codec) = &mut self.claude {
            codec.guard.seal();
        }
        // A failed probe is absence of cleanup proof; shutdown must still reap the owned process.
        let quiescent = self.quiesce().unwrap_or_else(|error| {
            self.cleanup_error("quiesce", &error);
            false
        });
        self.pipes.child.stdin.take();
        let ExitAttempt { mut method, exited } = self.stop_after_eof();
        if let Err(error) = &exited {
            self.cleanup_error("observe_exit", error);
        }
        let CleanupEvidence {
            quiescent,
            preinput,
        } = self.final_cleanup_evidence(quiescent);
        let mut confirmed = (quiescent || preinput) && matches!(exited, Ok(true));
        if !matches!(exited, Ok(true)) {
            method = ShutdownMethod::Forced;
        }
        let status = self.reap_owned(&exited, &mut confirmed);
        // A successful exit is required in addition to protocol cleanup. SIGTERM
        // and exit 0 alone do not establish that separately-grouped tools stopped.
        confirmed &= status.as_ref().is_some_and(|status| {
            status.success()
                || (self.claude_preinput_rejected()
                    && status.code() == Some(NATIVE_FAILURE_EXIT_CODE))
                || (status.code() == Some(NATIVE_FAILURE_EXIT_CODE)
                    && (preinput
                        || self
                            .claude
                            .as_ref()
                            .is_some_and(super::super::claude::Codec::terminal_error)))
        });
        if self.claude.is_some() && confirmed {
            // Native helpers can finish just after their leader. This is a
            // read-only wait for kernel-confirmed absence, never a signal to
            // a reaped PID or a quiet interval treated as cleanup proof.
            // Observation errors cannot certify absence, so keep the capacity reservation charged.
            confirmed = process::wait_group_absent(self.pipes.child.id()).unwrap_or_else(|error| {
                self.cleanup_error("group_absence", &error);
                false
            });
        }
        let report = Shutdown {
            method,
            confirmed,
            // Recovery may trust this durable receipt after the PID disappears.
            // Pre-input proof therefore needs every final check before recording it.
            quiescent: quiescent || (preinput && confirmed),
            reaped: status.is_some(),
            exit_code: status.and_then(|s| s.code()),
        };
        tracing::info!(name: "provider.exit",
             pid = self.pid(),
             method = report.method.as_str(),
             confirmed = report.confirmed,
             quiescent = report.quiescent,
             exit_code = report.exit_code);
        if self.pipes.stderr_bytes != 0 {
            tracing::info!(name: "provider.stderr_tail",
                 pid = self.pid(),
                 bytes = self.pipes.stderr_bytes,
                 code = "native_diagnostic_bytes",
                 decision = "discard_private_text");
        }
        self.closed = Some(report.clone());
        report
    }
}
impl Drop for Rpc {
    fn drop(&mut self) {
        self.close();
    }
}
