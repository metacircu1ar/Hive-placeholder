//! Bounded provider pipe writes, reads, buffering, and readiness waits.
//! Imperative drain loops preserve partial-write offsets, byte charges and frame order.
use super::{
    AsFd, BorrowedFd, Duration, Error, FRAME_LIMIT, IO_POLL_INTERVAL, IncomingFrame, Instant,
    PENDING_LIMIT, PIPE_DRAIN_BUDGET, PIPE_READ_BYTES, Read, Result, Rpc, Value, WRITE_TIMEOUT,
    Write, os, process, protocol,
};
use crate::runtime::api;
use crate::runtime::provider::claude::Codec;
use crate::runtime::provider::error::EXITED_BEFORE_CLOSE;
use std::io::ErrorKind;
use std::thread;

// Drain a typed preinput rejection after EPIPE before classifying startup failure.
const STARTUP_EXIT_DRAIN_GRACE: Duration = Duration::from_secs(1);
// Bound native-to-session translation work before another readiness iteration.
const TRANSLATION_FRAME_BUDGET: usize = 256;

impl Rpc {
    #[cfg(feature = "comb")]
    pub(crate) fn forget_comb_turn(&mut self, turn: &str) {
        if let Some(codec) = &mut self.claude {
            codec.forget_comb_turn(turn);
        }
    }
    #[cfg(feature = "comb")]
    pub(crate) fn forget_tool(&mut self, id: &Value) -> Result<()> {
        if let Some(codec) = &mut self.claude {
            codec.forget_tool(id)?;
        }
        Ok(())
    }
    pub(crate) fn send(&mut self, value: super::outgoing::OutgoingFrame) -> Result<()> {
        let result = self.send_inner(value);
        if result.as_ref().is_err_and(|e| {
            !matches!(e, Error::ProviderExited { .. }) || !self.claude_preinput_rejected()
        }) {
            self.protocol_uncertain = true;
        }
        result
    }
    fn send_inner(&mut self, value: super::outgoing::OutgoingFrame) -> Result<()> {
        let Some(codec) = &mut self.claude else {
            return self.write(api::value(value));
        };
        if self.protocol_uncertain {
            return Err(protocol());
        }
        let frames = codec.send(value)?;
        self.queue_output(frames)?;
        self.flush_output()?;
        Ok(())
    }
    pub(super) fn queue_output(&mut self, frames: Vec<Value>) -> Result<()> {
        if !self.outbox.frames.try_extend(
            frames.into_iter().map(|value| {
                let bytes = value.to_string().len();
                (value, bytes)
            }),
            PENDING_LIMIT,
        ) {
            return Err(Error::output_limit("Too many pending provider replies."));
        }
        Ok(())
    }
    pub(super) fn flush_output(&mut self) -> Result<()> {
        if self.protocol_uncertain {
            self.outbox.frames.clear();
            return Err(protocol());
        }
        if self.outbox.writing {
            return Ok(());
        }
        self.outbox.writing = true;
        let result = self.flush_queued();
        self.outbox.writing = false;
        result
    }
    fn flush_queued(&mut self) -> Result<()> {
        while let Some(frame) = self.outbox.frames.pop_front() {
            self.write(frame.clone())?;
            if let Some(codec) = &mut self.claude {
                codec.written(&frame);
            }
        }
        Ok(())
    }
    // Keep the outgoing frame owned until its bounded write finishes, including errors.
    pub(super) fn write(&mut self, value: Value) -> Result<()> {
        let mut bytes =
            serde_json::to_vec(&value).map_err(|cause| protocol().with_source(cause))?;
        if bytes.len() > FRAME_LIMIT {
            return Err(protocol());
        }
        bytes.push(b'\n');
        let deadline = Instant::now() + WRITE_TIMEOUT;
        let mut offset = 0;
        while offset < bytes.len() {
            if Instant::now() >= deadline {
                return Err(Error::provider_timeout(
                    "Provider stopped reading its input.",
                ));
            }
            match self
                .pipes
                .child
                .stdin
                .as_mut()
                .ok_or_else(protocol)?
                .write(&bytes[offset..])
            {
                Ok(0) => return Err(protocol()),
                Ok(n) => offset += n,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if let Some(value) = self.next()? {
                        let bytes = value.encoded_len();
                        if !self.pending.try_push(value, bytes, PENDING_LIMIT) {
                            return Err(Error::output_limit("Too many pending provider events."));
                        }
                    }
                    // Resume as soon as the provider drains input. A fixed sleep
                    // per full pipe caps throughput and can time out a healthy
                    // reader on a large escaped frame. Keep the short cap so
                    // output and Claude hook traffic continue to be serviced.
                    os::poll_writable(
                        self.pipes
                            .child
                            .stdin
                            .as_ref()
                            .ok_or_else(protocol)?
                            .as_fd(),
                        IO_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                    )
                    .map_err(|cause| protocol().with_source(cause))?;
                }
                Err(cause)
                    if cause.kind() == ErrorKind::BrokenPipe && self.claude_before_input() =>
                {
                    // EPIPE is a transport consequence; drained startup evidence
                    // decides the error.
                    return self.drain_claude_startup_exit();
                }
                Err(cause) => return Err(protocol().with_source(cause)),
            }
        }
        Ok(())
    }
    pub(crate) fn next(&mut self) -> Result<Option<IncomingFrame>> {
        if self.claude.is_some() && !self.outbox.writing {
            self.flush_output()
                .inspect_err(|_| self.protocol_uncertain = true)?;
            // Backpressure reads already observed these frames. They must be
            // delivered before any later native output, without observing twice.
            if let Some(frame) = self.pending.pop_front() {
                return Ok(Some(frame));
            }
        }
        let result = self.read_next();
        match &result {
            Ok(Some(value)) => self.observe(value),
            Err(Error::ProviderExited { .. }) if self.expected_claude_exit() => {}
            Err(_) => self.protocol_uncertain = true,
            _ => {}
        }
        result
    }
    pub(super) fn drain_claude_startup_exit(&mut self) -> Result<()> {
        // A missing resume may report its typed rejection and exit before our
        // first control write. Drain that evidence before classifying EPIPE.
        let deadline = Instant::now() + STARTUP_EXIT_DRAIN_GRACE;
        while Instant::now() < deadline {
            match self.read_checked_raw() {
                Ok(Some(value)) => {
                    let responses = self.claude.as_mut().ok_or_else(protocol)?.receive(value)?;
                    if !responses.is_empty() {
                        return Err(protocol());
                    }
                }
                Ok(None) => thread::sleep(IO_POLL_INTERVAL),
                Err(Error::ProviderExited { .. }) => {
                    return Err(Error::provider_exited(EXITED_BEFORE_CLOSE));
                }
                Err(error) => return Err(error),
            }
        }
        Err(protocol())
    }
    /// Sleep on pipes and a command wake descriptor; never on an agent poll timer.
    pub(crate) fn commit_branch(&mut self, turn: &str) -> Result<()> {
        if let Some(codec) = &mut self.claude {
            codec.commit_branch(turn)?;
        }
        Ok(())
    }
    pub(crate) fn wait(&self, wake: Option<BorrowedFd<'_>>, timeout: Duration) -> Result<()> {
        if !self.outbox.frames.is_empty()
            || self.pipes.buffer.contains(&b'\n')
            || self.claude.as_ref().is_some_and(Codec::buffered)
        {
            return Ok(());
        }
        let pipes = [
            (!self.pipes.stdout_eof).then(|| self.pipes.stdout.as_fd()),
            (!self.pipes.stderr_eof).then(|| self.pipes.stderr.as_fd()),
            wake,
        ];
        if let Some(codec) = &self.claude {
            let mut descriptors = codec.guard.fds();
            descriptors.extend(pipes.into_iter().flatten());
            return os::poll_many(
                &descriptors,
                codec
                    .guard
                    .wait_limit(codec.branch_wait(codec.steering_wait(timeout))),
            )
            .map_err(Error::state_io_from);
        }
        os::poll_readable(pipes, timeout).map_err(|cause| protocol().with_source(cause))
    }
    pub(super) fn read_next(&mut self) -> Result<Option<IncomingFrame>> {
        if let Some(codec) = &mut self.claude {
            codec.guard.service()?;
            let responses = codec.poll_branch().inspect_err(|_| {
                self.protocol_uncertain = true;
            })?;
            codec.settle();
            self.queue_output(responses)?;
        }
        for _ in 0..TRANSLATION_FRAME_BUDGET {
            if let Some(value) = self.claude.as_mut().and_then(Codec::pop) {
                return Ok(Some(IncomingFrame::parse(value).with_seq(self.read_seq)));
            }
            let Some(value) = self.read_checked_raw()? else {
                return Ok(None);
            };
            self.read_seq += 1;
            let Some(codec) = &mut self.claude else {
                // Displayed deltas never reach lifecycle observation, routing or
                // the writer; unwatched ones are dropped before any allocation.
                if let Some(wake) = &self.live
                    && let Some(kind) = super::super::live_tap::delta_kind(&value)
                {
                    if let super::super::live_tap::DeltaKind::Shown(kind) = kind
                        && wake.live.watched()
                        && let Some(delta) = super::super::live_tap::delta(kind, &value)
                    {
                        self.deltas.push(self.read_seq, delta);
                    }
                    continue;
                }
                return Ok(Some(IncomingFrame::parse(value).with_seq(self.read_seq)));
            };
            let responses = codec.receive(value).inspect_err(|_| {
                self.protocol_uncertain = true;
            })?;
            for delta in codec.take_deltas() {
                self.deltas.push(self.read_seq, delta);
            }
            self.queue_output(responses)?;
        }
        Ok(None)
    }
    pub(super) fn read_checked_raw(&mut self) -> Result<Option<Value>> {
        self.read_raw().inspect_err(|error| {
            if !matches!(error, Error::ProviderExited { .. })
                && let Some(codec) = &mut self.claude
            {
                codec.invalidate_preinput_proof();
            }
        })
    }
    fn drain_stderr(&mut self, bytes: &mut [u8]) -> Result<()> {
        // Drain diagnostics continuously without persisting raw auth/account data.
        for _ in 0..PIPE_DRAIN_BUDGET {
            match self.pipes.stderr.read(bytes) {
                Ok(0) => {
                    self.pipes.stderr_eof = true;
                    break;
                }
                Ok(count) => {
                    self.pipes.stderr_bytes = self.pipes.stderr_bytes.saturating_add(count as u64);
                    self.charge_capture(count)?;
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(cause) => return Err(protocol().with_source(cause)),
            }
        }
        Ok(())
    }
    fn buffered_frame(&mut self) -> Result<Option<Value>> {
        let Some(end) = self.pipes.buffer.iter().position(|b| *b == b'\n') else {
            return Ok(None);
        };
        let line: Vec<_> = self.pipes.buffer.drain(..=end).collect();
        serde_json::from_slice::<Value>(&line)
            .map(Some)
            .map_err(|cause| protocol().with_source(cause))
    }
    pub(super) fn read_raw(&mut self) -> Result<Option<Value>> {
        let mut bytes = [0; PIPE_READ_BYTES];
        self.drain_stderr(&mut bytes)?;
        if let Some(value) = self.buffered_frame()? {
            return Ok(Some(value));
        }
        for _ in 0..PIPE_DRAIN_BUDGET {
            match self.pipes.stdout.read(&mut bytes) {
                Ok(0) => {
                    self.pipes.stdout_eof = true;
                    break;
                }
                Ok(n) => {
                    self.charge_capture(n)?;
                    self.pipes.buffer.extend_from_slice(&bytes[..n]);
                    if self.pipes.buffer.len() > FRAME_LIMIT {
                        return Err(Error::output_limit(
                            "Provider frame exceeded the capture limit.",
                        ));
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(cause) => return Err(protocol().with_source(cause)),
            }
            if self.pipes.buffer.contains(&b'\n') {
                break;
            }
        }
        if let Some(value) = self.buffered_frame()? {
            return Ok(Some(value));
        }
        if self.pipes.stdout_eof || process::has_exited(&self.pipes.child)? {
            if !self.pipes.buffer.is_empty() {
                return Err(protocol());
            }
            return Err(Error::provider_exited(EXITED_BEFORE_CLOSE));
        }
        Ok(None)
    }
}
