//! Bounded input and output for owned provider helpers.
use super::{
    AsFd, CAPTURE_POLL_INTERVAL, CAPTURE_READ_BYTES, CAPTURE_SHUTDOWN_GRACE, Cancellation,
    CleanupReceipt, Command, Duration, Error, ExitStatus, Instant, LauncherCache, OUTPUT_LIMIT,
    Operation, OwnedGroup, Read, Result, Signal, Spawned, Stdio, Write, has_exited, os, spawn,
    wait_group_absent,
};
use serde_json::Value;
use std::{io::ErrorKind, mem, os::fd::BorrowedFd, thread};
const PROBE_TIMEOUT: &str = "Provider helper exceeded the probe timeout.";

const CANCELLED: &str = "Auth operation was cancelled.";

pub(crate) struct CapturedOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
}

pub(crate) struct Captured {
    owned: OwnedGroup,
    stdout: std::process::ChildStdout,
    stderr: std::process::ChildStderr,
    buffer: Vec<u8>,
    total: usize,
    deadline: Instant,
    cancellation: Cancellation,
}
impl Captured {
    pub(crate) fn start(
        launcher: &LauncherCache,
        command: &mut Command,
        operation: &mut Operation,
        timeout: Duration,
        cancellation: Cancellation,
    ) -> Result<Self> {
        Self::start_with_owner(launcher, command, operation, timeout, cancellation, None)
    }
    #[expect(
        clippy::expect_used,
        reason = "Successful spawn with group=true creates both configured piped output streams."
    )]
    pub(crate) fn start_with_owner(
        launcher: &LauncherCache,
        command: &mut Command,
        operation: &mut Operation,
        timeout: Duration,
        cancellation: Cancellation,
        owner: Option<BorrowedFd<'_>>,
    ) -> Result<Self> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let Spawned {
            mut child, receipt, ..
        } = spawn(launcher, command, operation, true, owner)?;
        let stdout = child.stdout.take().expect("captured child stdout is piped");
        let stderr = child.stderr.take().expect("captured child stderr is piped");
        let group = OwnedGroup::new(child, receipt, CleanupReceipt::GroupAbsence);
        for fd in [stdout.as_fd(), stderr.as_fd()] {
            if let Err(cause) = os::nonblocking(fd) {
                // OwnedGroup preserves receipt cleanup and logs any secondary
                // failures while the pipe configuration error remains primary.
                return Err(
                    Error::pipe_setup_failed("Cannot configure provider pipes.").with_source(cause)
                );
            }
        }
        Ok(Self {
            owned: group,
            stdout,
            stderr,
            buffer: Vec::new(),
            total: 0,
            deadline: Instant::now() + timeout,
            cancellation,
        })
    }
    fn tick(&mut self) -> Result<()> {
        self.read_available(true)
    }
    /// Read what the pipes hold now. `deadline` is false once the process has
    /// exited: its output is then complete, and draining it is not late.
    fn read_available(&mut self, deadline: bool) -> Result<()> {
        if self.cancellation.count() > 0 {
            return Err(Error::cancelled(CANCELLED));
        }
        if deadline && Instant::now() >= self.deadline {
            return Err(Error::timeout(PROBE_TIMEOUT));
        }
        let mut bytes = [0_u8; CAPTURE_READ_BYTES];
        for which in [true, false] {
            loop {
                let read = if which {
                    self.stdout.read(&mut bytes)
                } else {
                    self.stderr.read(&mut bytes)
                };
                match read {
                    Ok(0) => break,
                    Ok(n) => {
                        if self.cancellation.count() > 0 {
                            return Err(Error::cancelled(CANCELLED));
                        }
                        if deadline && Instant::now() >= self.deadline {
                            return Err(Error::timeout(PROBE_TIMEOUT));
                        }
                        self.total += n;
                        if self.total > OUTPUT_LIMIT {
                            return Err(Error::output_limit(
                                "Provider output exceeded the capture limit.",
                            ));
                        }
                        if which {
                            self.buffer.extend_from_slice(&bytes[..n]);
                        }
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(cause) => {
                        return Err(
                            Error::provider_io("Cannot read provider output.").with_source(cause)
                        );
                    }
                }
            }
        }
        Ok(())
    }
    pub(crate) fn send(&mut self, value: &Value) -> Result<()> {
        let mut message = serde_json::to_vec(value).map_err(|cause| {
            Error::protocol_error("Cannot encode provider request.").with_source(cause)
        })?;
        message.push(b'\n');
        self.owned
            .child
            .stdin
            .as_mut()
            .ok_or(Error::provider_io("Provider input closed."))?
            .write_all(&message)
            .map_err(|cause| {
                Error::provider_io("Cannot write provider request.").with_source(cause)
            })
    }
    pub(crate) fn reply(&mut self, id: u64) -> Result<Value> {
        loop {
            let message = self.next_json()?;
            if message.get("id").and_then(Value::as_u64) == Some(id)
                && message.get("method").is_none()
                && (message.get("result").is_some() || message.get("error").is_some())
            {
                return Ok(message);
            }
        }
    }
    pub(crate) fn next_json(&mut self) -> Result<Value> {
        loop {
            self.tick()?;
            if let Some(newline) = self.buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.buffer.drain(..=newline).collect();
                return serde_json::from_slice(&line)
                    .map_err(|cause| malformed().with_source(cause));
            }
            if has_exited(&self.owned.child)? {
                return Err(Error::provider_failed("Provider exited before responding."));
            }
            thread::sleep(CAPTURE_POLL_INTERVAL);
        }
    }
    pub(crate) fn output(&mut self) -> Result<CapturedOutput> {
        self.owned.child.stdin.take();
        loop {
            // An exit observed before reading wins over a deadline reached
            // since: the process finished in time.
            let exited = has_exited(&self.owned.child)?;
            self.read_available(!exited)?;
            if exited {
                // The unreaped group leader prevents PGID reuse during cleanup.
                self.owned.signal(Signal::KILL);
                self.read_available(false)?;
                let status = self.owned.child.wait().map_err(|cause| {
                    Error::process_wait_failed("Cannot reap the provider helper.")
                        .with_source(cause)
                })?;
                self.owned.reaped = true;
                self.owned
                    .receipt
                    .finish(wait_group_absent(self.owned.child.id())?)?;
                return Ok(CapturedOutput {
                    status,
                    stdout: mem::take(&mut self.buffer),
                });
            }
            thread::sleep(CAPTURE_POLL_INTERVAL);
        }
    }
    /// Give App Server EOF and a short opportunity to flush provider-owned files.
    /// A false result leaves forced group cleanup to Drop without invalidating a reply.
    pub(crate) fn shutdown(&mut self) -> Result<bool> {
        Ok(self.finish_with_grace(CAPTURE_SHUTDOWN_GRACE)?.is_some())
    }
    pub(crate) fn finish_with_grace(&mut self, grace: Duration) -> Result<Option<CapturedOutput>> {
        self.deadline = self.deadline.min(Instant::now() + grace);
        match self.output() {
            Ok(output) => Ok(Some(output)),
            Err(Error::Timeout { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }
}
pub(crate) fn malformed() -> Error {
    Error::malformed_provider_output("Provider status was not recognized; no login was attempted.")
}

#[cfg(test)]
mod tests;
