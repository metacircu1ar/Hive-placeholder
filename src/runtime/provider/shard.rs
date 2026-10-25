//! Bounded provider shards. One I/O thread owns each process; agent actors are
//! async tasks. RPC requests are pipelined, and control has reserved capacity.
use std::path::Path;
mod dispatch;
mod driver;
mod init;
mod launch;
mod shutdown;
mod state;
use launch::{bootstrap, validate_account};
use state::Occupancy;

#[cfg(test)]
pub(crate) mod corpus_support;
#[cfg(test)]
mod protocol_corpus;

use super::error::{protocol, unavailable};
use super::protocol::{ClientMethod, IncomingFrame, Model, Response, outgoing};
use super::{
    pool::Pool,
    registry::Registry,
    rpc::{Rpc, Shutdown},
};
use crate::{
    error::{Error, Result},
    runtime::{
        domain::{EventKind, ProviderEventKind},
        locks::MutexExt,
        scheduler::Activity,
    },
    state::{Lock, RuntimeLease},
};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    io::{Read, Write},
    os::unix::{io::AsFd, net::UnixStream},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Notify, OnceCell, mpsc, oneshot, watch};
use uuid::Uuid;

// Maximum normal requests in flight on one provider connection.
const NORMAL_RPC_WINDOW: usize = 64;
// Reserve slots above the normal window so priority control can make progress.
const RESERVED_PRIORITY_SLOTS: usize = 8;
const PRIORITY_RPC_WINDOW: usize = NORMAL_RPC_WINDOW + RESERVED_PRIORITY_SLOTS;
// Maximum tracked requests, counting timed-out tombstones as well as live requests.
const REQUEST_TOMBSTONE_LIMIT: usize = 200;
// Limit commands per iteration so frame handling gets another turn.
const COMMAND_DRAIN_BUDGET: i32 = 128;
// Limit frames per iteration so command handling gets another turn.
const FRAME_DRAIN_BUDGET: i32 = 256;
// How long a request may wait for the provider's acknowledgement before it fails.
const REQUEST_ACK_TIMEOUT: Duration = Duration::from_secs(30);
// Revalidate the account before attaching to an empty connection older than this.
const ACCOUNT_REVALIDATION_AGE: Duration = Duration::from_secs(30);
// Wait for unresolved lifecycle acknowledgements after the last session releases the shard.
const SHARD_CLOSE_GRACE: Duration = Duration::from_secs(10);
// Maximum poll wait when the shard has no pending request deadline.
const SHARD_IDLE_POLL_TIMEOUT: Duration = Duration::from_secs(3600);
// Stop catalog discovery if the provider keeps returning more pages.
const MODEL_PAGE_BUDGET: i32 = 10;
// Maximum normal commands queued before senders must wait.
const NORMAL_QUEUE_CAPACITY: usize = 256;
// Keep a separate bounded queue so normal traffic cannot block priority commands.
const PRIORITY_QUEUE_CAPACITY: usize = 64;
// Keep cleanup commands that need no acknowledgement separate from waiting RPC calls.
const ESSENTIAL_QUEUE_CAPACITY: usize = 64;
// Drain coalesced one-byte notifications without reading an unbounded wake stream.
const WAKE_READ_BYTES: usize = 256;
// Maximum queued frame bytes charged to one agent before its route fails.
// Native input item echoes may contain an entire joined reply twice (started/completed).
const AGENT_BYTES: usize = 2 * crate::limits::JOIN_TRANSPORT_BYTES;
// Maximum queued frame bytes charged to a provider shard before it drains.
const SHARD_BYTES: usize = 4 * crate::limits::JOIN_TRANSPORT_BYTES;
const _: () = assert!(NORMAL_RPC_WINDOW < PRIORITY_RPC_WINDOW);
const _: () = assert!(PRIORITY_RPC_WINDOW < REQUEST_TOMBSTONE_LIMIT);
const _: () = assert!(AGENT_BYTES <= SHARD_BYTES);

struct Charge {
    local: Arc<AtomicUsize>,
    global: Arc<AtomicUsize>,
    bytes: usize,
}
impl Drop for Charge {
    fn drop(&mut self) {
        self.local.fetch_sub(self.bytes, Ordering::Relaxed);
        self.global.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}
pub(super) struct Frame {
    pub(super) value: IncomingFrame,
    _charge: Charge,
}
pub(super) struct Route {
    pub(super) attach_cancelled: Arc<AtomicBool>,
    pub(super) tx: mpsc::Sender<Frame>,
    pub(super) bytes: Arc<AtomicUsize>,
    pub(super) failure: watch::Sender<Option<Error>>,
}
impl Route {
    fn send(&self, frame_value: IncomingFrame, total: &Arc<AtomicUsize>) -> Result<()> {
        if self.attach_cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
        let bytes = frame_value.encoded_len();
        let local = self.bytes.fetch_add(bytes, Ordering::Relaxed) + bytes;
        let global = total.fetch_add(bytes, Ordering::Relaxed) + bytes;
        let frame = Frame {
            value: frame_value,
            _charge: Charge {
                local: self.bytes.clone(),
                global: total.clone(),
                bytes,
            },
        };
        if global > SHARD_BYTES {
            return Err(Error::shard_output_limit(
                "Provider event buffers exceeded the shared byte budget.",
            ));
        }
        if local > AGENT_BYTES {
            return Err(Error::output_limit(
                "Agent event buffer exceeded its byte budget.",
            ));
        }
        match self.tx.try_send(frame) {
            Ok(()) => Ok(()),
            Err(_) if self.attach_cancelled.load(Ordering::Acquire) => {
                crate::observability::lost_reply(crate::observability::ReplyOwner::Provider, None);
                Ok(())
            }
            Err(cause) => Err(
                Error::output_limit("Agent event queue is not being drained.")
                    .with_try_send_source(cause),
            ),
        }
    }
}
pub(super) enum Cmd {
    Attach(String, Route, oneshot::Sender<Result<Started>>),
    Call(
        String,
        super::protocol::outgoing::OutgoingRequest,
        oneshot::Sender<Result<Response>>,
    ),
    Send(
        super::protocol::outgoing::OutgoingFrame,
        oneshot::Sender<Result<()>>,
    ),
    #[cfg(feature = "comb")]
    ForgetTool(Value, oneshot::Sender<Result<()>>),
    #[cfg(feature = "comb")]
    ForgetCombTurn(String, oneshot::Sender<()>),
    CommitBranch(String, oneshot::Sender<Result<()>>),
    Inspect(String, oneshot::Sender<Result<Option<String>>>),
    TerminalDrain(
        String,
        String,
        Option<u64>,
        oneshot::Sender<Result<super::cleanup::Snapshot>>,
    ),
    Retire(String, oneshot::Sender<Result<()>>),
    Quarantine(String, oneshot::Sender<()>),
    Release {
        agent: String,
        last: bool,
        clean: bool,
        reply: oneshot::Sender<Shutdown>,
    },
}
#[derive(Clone)]
pub(crate) struct Started {
    pub pid: u32,
    pub before: u64,
    pub after: u64,
}
struct Pending {
    agent: String,
    method: ClientMethod,
    reply: Option<oneshot::Sender<Result<Response>>>,
    deadline: Option<Instant>,
    thread: Option<String>,
}

pub(crate) struct Shard {
    pub(super) id: String,
    pub(super) auth: String,
    pub(super) process_options: String,
    pub(super) key: Option<crate::state::KeyAuthority>,
    pub(super) backend: super::Backend,
    pub(super) compatibility: Option<crate::provider_support::Evidence>,
    pub(super) created: Instant,
    occupancy: Mutex<Occupancy>,
    empty_attach_check: AtomicBool,
    abandoned_attach: AtomicBool,
    pub(super) ready_since: std::sync::OnceLock<Instant>,
    starting: AtomicBool,
    // Method-not-found is process-wide; supported/unknown sessions still probe.
    steering_unsupported: AtomicBool,
    last_used: AtomicU64,
    pub(super) lease: Arc<RuntimeLease>,
    normal: mpsc::Sender<Cmd>,
    priority: mpsc::Sender<Cmd>,
    essential: mpsc::Sender<Cmd>,
    wake: UnixStream,
    draining: AtomicBool,
    exited: AtomicBool,
    final_events: AtomicBool,
    capacity_released: AtomicBool,
    pub(super) process: Mutex<Option<crate::state::ProcessIdentity>>,
    stop: AtomicBool,
    pub(super) catalog: OnceCell<Result<Vec<Model>>>,
    binary_identity: std::sync::OnceLock<super::catalogs::BinaryIdentity>,
    pub(super) report: Mutex<Option<Shutdown>>,
    pub(super) done: Notify,
}
impl Shard {
    fn observe_binary(&self, command: &Command) {
        if let Some(identity) = crate::observability::optional(
            super::catalogs::BinaryIdentity::read(Path::new(command.get_program())),
            "provider_binary_identity",
        ) {
            let _ = self.binary_identity.set(identity);
        }
    }
    pub(super) fn commands_closed(&self) -> bool {
        self.normal.is_closed()
    }
    pub(super) async fn wait_exited(&self) {
        loop {
            let done = self.done.notified();
            tokio::pin!(done);
            done.as_mut().enable();
            if self.exited() {
                break;
            }
            done.await;
        }
    }
    pub(super) async fn command(&self, cmd: Cmd, priority: bool) -> Result<()> {
        let sender = if priority && !matches!(&cmd, Cmd::Call(..)) {
            &self.essential
        } else if priority {
            &self.priority
        } else {
            &self.normal
        };
        let method = match &cmd {
            Cmd::Call(_, request, _) => Some(request.method()),
            _ => None,
        };
        sender.send(cmd).await.map_err(|cause| {
            let error = unavailable().with_send_source(cause);
            match method {
                Some(method) => method.submission_error(self.backend.multiplexed(), error),
                None => error,
            }
        })?;
        self.poke()
    }
}

fn thread_key(frame_value: &IncomingFrame) -> Option<&str> {
    frame_value.params().thread_key()
}
fn in_flight(pending: &HashMap<u64, Pending>) -> usize {
    pending
        .values()
        .filter(|r| r.reply.is_some() || r.deadline.is_some())
        .count()
}

#[derive(PartialEq, Eq, Hash)]
struct InterruptedTurn {
    thread: String,
    turn: String,
}

struct ShardLoop<'a> {
    pool: &'a Pool,
    shard: &'a Arc<Shard>,
    rpc: &'a mut Rpc,
    started: &'a Started,
    wake: &'a mut UnixStream,
    normal: &'a mut mpsc::Receiver<Cmd>,
    priority: &'a mut mpsc::Receiver<Cmd>,
    essential: &'a mut mpsc::Receiver<Cmd>,
    // Match the original local-variable drop order, including unwind paths.
    closing: Option<Instant>,
    sink_threads: HashSet<String>,
    interrupted: HashSet<InterruptedTurn>,
    release_replies: Vec<oneshot::Sender<Shutdown>>,
    bytes: Arc<AtomicUsize>,
    pending: HashMap<u64, Pending>,
    threads: HashMap<String, String>,
    routes: HashMap<String, Route>,
    notices: HashSet<String>,
    limits: crate::runtime::auth_fallback::signals::CodexLimits,
}

#[cfg(test)]
mod admission_tests;

#[cfg(test)]
mod attachment_tests;

#[cfg(test)]
mod lifecycle_tests;

#[cfg(test)]
mod live_tests;
