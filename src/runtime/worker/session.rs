//! Provider session setup, admission and turn event handling.
mod attachments;
#[cfg(feature = "comb")]
mod comb;
#[cfg(test)]
mod corpus_support;
mod frames;
mod models;
mod startup;
mod turn_loop;
use crate::runtime::provider::protocol::validate_model;
use frames::handle;
#[cfg(test)]
use frames::{dispatch_tool, finish_turn};
pub(super) use startup::{SessionInputs, execute_session, identity};
#[cfg(test)]
mod protocol_corpus;
mod steering;
use super::ExecutionHandle;
use crate::error::{Error, Result};
use crate::runtime::provider::protocol::{
    ClientMethod, IncomingFrame, Model, ParamsRef, Response, ServerMethod, outgoing,
};
use crate::runtime::{
    api::{Input, RequestEnvelope, validate_tool},
    domain::{Agent, AgentState, Control, EventKind, MessageState, OptionsSelection},
    provider::Session,
    response,
    store::Message,
};
use models::TurnAdmission;
use serde_json::Value;
use std::{
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

// How long the worker waits for interruption confirmation before reporting a timeout.
const INTERRUPT_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(10);
// Longest a single turn may run before the worker fails it.
const TURN_TIMEOUT: Duration = Duration::from_secs(1800);
// The public timeout message states thirty minutes.
const _: () = assert!(TURN_TIMEOUT.as_secs() == 30 * 60);
// Effectively unbounded select! wait when no deadline is active.
const NO_DEADLINE_WAIT: Duration = Duration::from_secs(86400 * 365);

type Admission = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = std::result::Result<OwnedSemaphorePermit, tokio::sync::AcquireError>,
            > + Send,
    >,
>;
enum AdmissionEvent {
    Permit(std::result::Result<OwnedSemaphorePermit, tokio::sync::AcquireError>),
    Frame(Result<IncomingFrame>),
    Wake,
}
enum SessionEvent {
    Frame(Result<IncomingFrame>),
    Steering(steering::Acknowledgement),
    #[cfg(feature = "comb")]
    Comb(usize, crate::runtime::comb::Result<Value>),
    Wake,
    Deadline,
}
struct ActiveTurn {
    message: Message,
    turn_id: String,
    started: Instant,
}

struct AdmissionReadiness {
    waiting: bool,
    placement: usize,
}

/// Only the decisions used between hub turns; refreshed behind the execution fence.
struct SessionState {
    paused: bool,
    terminal: bool,
    branch_unbound: bool,
    control: Option<Control>,
    waiting_reason: Option<crate::runtime::auth_fallback::WaitingReason>,
    branch_hold: u64,
}
impl From<&Agent> for SessionState {
    fn from(agent: &Agent) -> Self {
        Self {
            paused: agent.paused,
            terminal: agent.terminal(),
            branch_unbound: agent.branch_unbound(),
            control: agent.control,
            waiting_reason: agent.waiting_reason,
            branch_hold: agent.branch_hold,
        }
    }
}

struct SessionLoop<'a> {
    /// The most recent selection accepted by a turn on this native thread.
    /// This remains distinct from re-established session config so sticky turn
    /// options can be explicitly reset on the first resumed turn.
    applied_turn_selection: OptionsSelection,
    selection: OptionsSelection,
    relaunch: bool,
    execution: ExecutionHandle<'a>,
    state: SessionState,
    rpc: &'a mut Session,
    shutdown: &'a std::sync::atomic::AtomicBool,
    notify: &'a Notify,
    turns: &'a Arc<Semaphore>,
    permit: &'a mut Option<OwnedSemaphorePermit>,
    idle_seconds: u64,
    thread_id: &'a str,
    // Admission must be cancelled before active-message storage is dropped.
    idle_at: Instant,
    stop_deadline: Option<Instant>,
    interrupt_sent: bool,
    admission: Option<Admission>,
    active: Option<ActiveTurn>,
    pending_steering: Vec<steering::Pending>,
    #[cfg(feature = "comb")]
    pending_comb: Vec<comb::Pending>,
}

#[cfg(test)]
mod model_tests;

#[cfg(test)]
mod startup_tests;

#[cfg(test)]
mod replay_tests;
