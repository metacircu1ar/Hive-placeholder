//! Provider transport ownership, bounded protocol state, and native process lifecycle.
mod authentication;
mod io;
mod launch;
mod model_list;
pub(crate) use model_list::ModelLookup;
mod requests;
mod shutdown;
mod terminal_drain;
#[cfg(test)]
mod thread_state_tests;
mod threads;

#[cfg(test)]
mod protocol_corpus;
#[cfg(test)]
mod steering_tests;
#[cfg(test)]
mod terminal_drain_tests;
#[cfg(test)]
pub(super) mod test_support;

use super::super::domain::ProviderTurnStatus;
use super::error::transport_protocol as protocol;
use super::protocol::{ClientMethod, IncomingFrame, Response, ServerMethod, outgoing};
use super::queue::{ByteQueue, Limits};
use crate::{
    error::{Error, Result},
    os, process,
};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    io::{Read, Write},
    os::unix::io::{AsFd, AsRawFd, BorrowedFd},
    process::{Child, ChildStderr, ChildStdout, Command, Stdio},
    time::{Duration, Instant},
};

// How long a blocked provider write may take before the connection is marked uncertain.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
// Interval between blocked-write retries and provider-exit checks.
const IO_POLL_INTERVAL: Duration = Duration::from_millis(10);
// How long a synchronous RPC waits for its response before failing.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
// Maximum frames retained while waiting for a write or another RPC's response.
const PENDING_FRAME_LIMIT: usize = 1024;
// Maximum encoded bytes retained while waiting for a write or another RPC's response.
const PENDING_BYTES_LIMIT: usize = 2 * crate::limits::JOIN_TRANSPORT_BYTES;
const PENDING_LIMIT: Limits = Limits {
    items: PENDING_FRAME_LIMIT,
    bytes: PENDING_BYTES_LIMIT,
};
// Maximum accepted thread or turn ID length in bytes.
const PROTOCOL_ID_BYTES: usize = 1024;
// Maximum early turn notifications retained while a thread is being opened.
const EARLY_TURN_LIMIT: usize = 8;
// Read pipes in bounded chunks while preserving partial frames.
const PIPE_READ_BYTES: usize = 8192;
// Limit reads from each provider pipe per pass so other work can proceed.
const PIPE_DRAIN_BUDGET: i32 = 32;

// Maximum encoded frame size accepted from or sent to the provider.
const FRAME_LIMIT: usize = crate::limits::JOIN_TRANSPORT_BYTES;
pub(crate) struct Rpc {
    pub(crate) initialization: Option<Value>,
    claude: Option<super::claude::Codec>,
    outbox: Outbox,
    pipes: Pipes,
    pub pending: ByteQueue<IncomingFrame>,
    next_id: u64,
    closed: Option<Shutdown>,
    threads: HashMap<String, ThreadState>,
    requests: HashMap<u64, Request>,
    opening: usize,
    pub(super) account_identity: Option<String>,
    protocol_uncertain: bool,
    // Monotonic summary keeps request admission O(1); per-thread evidence is never cleared.
    has_quarantined: bool,
    expected_method: crate::config::Method,
    private_bootstrap: bool,
    // Present when this process streams deltas for watched agents.
    live: Option<std::sync::Arc<crate::runtime::hub::Wake>>,
    pub(crate) deltas: super::live_tap::Deltas,
    // Raw provider lines read so far; orders deltas against deferred frames.
    read_seq: u64,
}

#[derive(Default)]
struct Outbox {
    frames: ByteQueue<Value>,
    capture_remaining: Option<usize>,
    writing: bool,
}
struct Pipes {
    // Preserve native process/descriptor destruction order within Rpc.
    child: Child,
    stdout: ChildStdout,
    stderr: ChildStderr,
    stdout_eof: bool,
    stderr_eof: bool,
    stderr_bytes: u64,
    buffer: Vec<u8>,
}
#[derive(Default)]
struct ThreadState {
    // None means the conversation was retired from active cleanup, not forgotten.
    // Other evidence survives retirement until the entire provider process drops.
    turn: Option<Turn>,
    early: Vec<IncomingFrame>,
    completed: Option<String>,
    drain: Option<terminal_drain::Drain>,
    drained: Option<String>,
    late_after_drain: bool,
    early_item: Option<String>,
    quarantined: bool,
}
#[derive(Clone)]
enum Turn {
    Idle,
    Submitting,
    Active(String),
}
struct Request {
    method: ClientMethod,
    thread: Option<String>,
    expected_turn: Option<String>,
    fork_cutoff: Option<String>,
}
crate::runtime::domain::domain_enum!(ShutdownMethod {
    NotStarted => "not_started", SessionReleased => "session_released", Uncertain => "uncertain",
    Eof => "eof", Sigterm => "sigterm", Forced => "forced",
});

#[derive(Clone, Serialize)]
pub(crate) struct Shutdown {
    pub method: ShutdownMethod,
    pub confirmed: bool,
    pub quiescent: bool,
    pub reaped: bool,
    pub exit_code: Option<i32>,
}
impl Rpc {
    pub(crate) fn capture_limit(&mut self, bytes: usize) {
        self.outbox.capture_remaining = Some(bytes);
    }
    fn charge_capture(&mut self, count: usize) -> Result<()> {
        if let Some(left) = self.outbox.capture_remaining.as_mut() {
            *left = left.checked_sub(count).ok_or_else(|| {
                Error::output_limit("Native canary output exceeded its capture limit.")
            })?;
        }
        Ok(())
    }
    pub(crate) fn pid(&self) -> u32 {
        self.pipes.child.id()
    }
    /// Stream displayed deltas to watched agents; call before initialization.
    pub(crate) fn stream_live(&mut self, wake: std::sync::Arc<crate::runtime::hub::Wake>) {
        if let Some(codec) = &mut self.claude {
            codec.stream_live(wake.clone());
        }
        self.live = Some(wake);
    }
    pub(crate) fn uncertain(&self) -> bool {
        self.protocol_uncertain
    }
    pub(crate) fn buffered(&self) -> bool {
        // Deltas read during a call or after a budgeted drain still need a pass.
        !self.pending.is_empty()
            || !self.deltas.is_empty()
            || !self.outbox.frames.is_empty()
            || self.pipes.buffer.contains(&b'\n')
            || self
                .claude
                .as_ref()
                .is_some_and(super::claude::Codec::buffered)
    }
    pub(crate) fn poison(&mut self) {
        self.protocol_uncertain = true;
    }
    pub(crate) fn quarantine(&mut self, thread: &str) {
        if let Some(state) = self.threads.get_mut(thread)
            && (state.turn.is_some() || state.completed.is_some())
        {
            state.quarantined = true;
            self.has_quarantined = true;
        } else {
            // Unknown execution cannot be attributed safely to one conversation.
            self.protocol_uncertain = true;
        }
    }
    pub(crate) fn claude_account_identity(&self) -> Option<&str> {
        self.claude
            .as_ref()
            .and_then(super::claude::Codec::account_identity)
    }
    pub(crate) fn account_identity(&self) -> Option<&str> {
        self.account_identity.as_deref()
    }
    pub(crate) fn claude_subscription(&self) -> Option<bool> {
        self.claude.as_ref().map(super::claude::Codec::subscription)
    }
    pub(super) fn expected_claude_exit(&self) -> bool {
        self.claude_preinput_rejected()
            || (!self.unresolved_lifecycle()
                && self
                    .claude
                    .as_ref()
                    .is_some_and(|codec| codec.terminal_error() && codec.quiescent()))
    }
    pub(super) fn claude_preinput_rejected(&self) -> bool {
        self.claude
            .as_ref()
            .is_some_and(super::claude::Codec::preinput_rejected)
    }
    pub(super) fn claude_before_input(&self) -> bool {
        self.claude
            .as_ref()
            .is_some_and(super::claude::Codec::before_input)
    }
    pub(crate) fn unresolved_lifecycle(&self) -> bool {
        self.opening > 0
            || self
                .threads
                .values()
                .any(|state| matches!(state.turn, Some(Turn::Submitting)))
    }
}

#[cfg(test)]
mod claude_tests;

#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used)] // Stub-provider fixtures stop immediately on setup failure.
mod live_tests;
#[cfg(test)]
mod logging_tests;
#[cfg(test)]
mod write_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;
    use std::os::unix::net::UnixStream;

    #[test]
    fn eof_pipes_are_omitted_from_poll_but_command_wakes_remain_active() {
        let launcher = crate::launch::test_cache().unwrap();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exec 1>&- 2>&-; read value; exit 0"]);
        let mut rpc = Rpc::start(&launcher, command, vec![]).unwrap();
        crate::test_support::wait_until(Duration::from_secs(5), Duration::from_millis(1), || {
            if rpc.pipes.stdout_eof && rpc.pipes.stderr_eof {
                return Some(());
            }
            let _ = rpc.read_next();
            None
        });
        let start = Instant::now();
        rpc.wait(None, Duration::from_millis(20)).unwrap();
        assert!(start.elapsed() >= Duration::from_millis(15));

        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(&[1]).unwrap();
        let start = Instant::now();
        rpc.wait(Some(reader.as_fd()), Duration::from_secs(5))
            .unwrap();
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(rpc.close().confirmed);
    }

    #[test]
    fn absent_inherited_descriptor_retains_spawn_error_and_errno() {
        let launcher = crate::launch::test_cache().unwrap();
        let file = tempfile::tempfile().unwrap();
        let error = Rpc::start(
            &launcher,
            Command::new("/bin/sh"),
            vec![Some(file.as_fd()), None],
        )
        .err()
        .unwrap();
        assert_eq!(error.code(), "spawn_failed");
        assert_eq!(
            error.message(),
            "Cannot start the trusted provider launcher."
        );
        let cause = error
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap();
        assert_eq!(
            cause.raw_os_error(),
            Some(rustix::io::Errno::BADF.raw_os_error())
        );
    }
}
