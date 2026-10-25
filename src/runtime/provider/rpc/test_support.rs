//! Captured provider transport and lifecycle setup for RPC and shard tests.
use super::super::protocol::{
    IncomingFrame, Response,
    outgoing::{self, OutgoingRequest},
};
use super::{Rpc, Shutdown, Turn};
use crate::{error::Result, launch::LauncherCache, test_support::wait_until};
use serde_json::{Value, json};
use std::{process::Command, sync::Arc, time::Duration};

pub(crate) struct RpcHarness {
    pub rpc: Rpc,
    output: tempfile::NamedTempFile,
    _launcher: Arc<LauncherCache>,
}

impl RpcHarness {
    pub(crate) fn new(launcher: Arc<LauncherCache>) -> Self {
        let output = tempfile::NamedTempFile::new().unwrap();
        let mut command = Command::new("/bin/sh");
        // Keep the provider stdout pipe open while cat captures requests in the file.
        // Otherwise idle reads see EOF and cannot exercise the acknowledgement timeout.
        command
            .args(["-c", "exec 3>&1; exec cat > \"$1\"", "corpus"])
            .arg(output.path());
        let rpc = Rpc::start(&launcher, command, vec![]).unwrap();
        Self {
            rpc,
            output,
            _launcher: launcher,
        }
    }

    pub(crate) fn active(launcher: Arc<LauncherCache>) -> Self {
        let mut harness = Self::new(launcher);
        harness.open();
        let id = harness.begin(outgoing::turn_input("thread", "")).unwrap();
        harness
            .response(&json!({"id":id,"result":{"turn":{"id":"turn"}}}))
            .unwrap();
        harness
    }

    pub(crate) fn open(&mut self) {
        let id = self.begin(outgoing::thread_start(None, None)).unwrap();
        self.response(&json!({"id":id,"result":{"thread":{"id":"thread"}}}))
            .unwrap();
    }

    // Exercise the same typed request construction as production callers.
    pub(crate) fn begin(&mut self, request: OutgoingRequest) -> Result<u64> {
        self.rpc.begin_request(request)
    }

    pub(crate) fn response(&mut self, value: &Value) -> Result<Value> {
        self.rpc
            .finish_response(&IncomingFrame::parse(value.clone()))
            .map(Response::into_raw)
    }

    pub(crate) fn ingest(&mut self, value: &Value) -> Result<Value> {
        self.rpc
            .pipes
            .buffer
            .extend_from_slice(value.to_string().as_bytes());
        self.rpc.pipes.buffer.push(b'\n');
        self.rpc.next().map(|frame| frame.unwrap().into_raw())
    }

    pub(crate) fn ingest_bytes(&mut self, bytes: &[u8]) -> Result<Option<Value>> {
        self.rpc.pipes.buffer.extend_from_slice(bytes);
        self.rpc
            .next()
            .map(|frame| frame.map(IncomingFrame::into_raw))
    }

    pub(crate) fn state(&self, thread: &str) -> &'static str {
        match self.rpc.turn(thread) {
            None => "absent",
            Some(Turn::Idle) => "idle",
            Some(Turn::Submitting) => "submitting",
            Some(Turn::Active(_)) => "active",
        }
    }

    pub(crate) fn outstanding(&self, id: u64) -> bool {
        self.rpc.requests.contains_key(&id)
    }

    pub(crate) fn pending(&mut self) -> Option<Value> {
        self.rpc.pending.pop_front().map(IncomingFrame::into_raw)
    }

    pub(crate) fn call_queued(&mut self, frames: &[Value]) -> Result<Value> {
        assert!(self.rpc.pending.try_extend(
            frames.iter().cloned().map(|raw| {
                let frame = IncomingFrame::parse(raw);
                let bytes = frame.encoded_len();
                (frame, bytes)
            }),
            super::PENDING_LIMIT
        ));
        self.rpc
            .call_with_timeout(
                crate::runtime::provider::protocol::outgoing::models(100),
                Duration::from_secs(1),
            )
            .map(Response::into_raw)
    }

    pub(crate) fn outgoing(&self, count: usize) -> Vec<Value> {
        wait_until(Duration::from_secs(3), Duration::from_millis(1), || {
            let bytes = std::fs::read_to_string(self.output.path()).unwrap();
            let frames: Vec<Value> = bytes
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            (frames.len() >= count).then_some(frames)
        })
    }
}

impl Drop for RpcHarness {
    fn drop(&mut self) {
        // The child only captures writes; simulated protocol state cannot be quiesced by it.
        // Kill/wait also runs during assertion unwinding, before Rpc's normal Drop can signal.
        let _ = self.rpc.pipes.child.kill();
        let status = self
            .rpc
            .pipes
            .child
            .wait()
            .expect("reap corpus capture process");
        self.rpc.closed = Some(Shutdown {
            method: crate::runtime::provider::ShutdownMethod::Eof,
            confirmed: false,
            quiescent: false,
            reaped: true,
            exit_code: status.code(),
        });
    }
}
