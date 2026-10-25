//! Translate Claude's native evidence into the shared session event contract.
//! Local session reservation is distinct from native input acknowledgement.
use crate::runtime::provider::queue::ByteQueue;
use crate::shell::guard::ShellGuard;
mod assistant;
mod branch;
mod controls;
mod incoming;
mod limits;
mod outgoing;
pub(in crate::runtime) use outgoing::user_frame;
mod steering;
use super::{Initialization, SessionConfig, catalog, wire};
use crate::runtime::api;
use crate::runtime::provider::error::native_protocol as protocol;
use crate::{
    error::{Error, Result},
    runtime::{
        domain::ProviderTurnStatus,
        provider::protocol::{ClientMethod, ServerMethod},
    },
};
use limits::ASSISTANT_TEXT_LIMIT;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};
pub(crate) use steering::{MAX_HELD, Permit, ReceiptMode};
use uuid::Uuid;
use wire::emitted;

pub(crate) struct Codec {
    // Keep the native guard ahead of configuration and in-memory protocol state:
    // its destruction must fence native work before the remaining fields drop.
    pub(crate) guard: crate::shell::guard::ShellGuard,
    config: Arc<SessionConfig>,
    branch: Option<branch::Gate>,
    initialization: Option<Initialization>,
    callbacks: Callbacks,
    tools: ToolTracker,
    steering: steering::Steering,
    steering_permit: Option<super::SteeringPermit>,
    frames: VecDeque<Value>,
    echoes: ReplyEchoes,
    turn: TurnState,
    opened: bool,
    verified: bool,
    expected_method: crate::config::Method,
    // Live streaming for watched agents; the latest native message keys deltas.
    live: Option<Arc<crate::runtime::hub::Wake>>,
    stream_message: Option<String>,
    deltas: Vec<crate::runtime::provider::live_tap::Delta>,
}

#[derive(Default)]
struct Callbacks {
    controls: HashMap<String, (u64, ClientMethod)>,
    requests: HashMap<u64, (String, Value)>,
    cancelled: HashSet<u64>,
    next_id: u64,
    #[cfg(feature = "comb")]
    comb_turns: HashMap<u64, String>,
    #[cfg(feature = "comb")]
    retired_comb_turn: Option<String>,
}
impl Callbacks {
    fn matching_requests(&self, matches: impl Fn(&str, &Value) -> bool) -> Vec<u64> {
        self.requests
            .iter()
            .filter(|(_, (native, mcp))| matches(native, mcp))
            .map(|(id, _)| *id)
            .collect()
    }
}

#[derive(Default)]
struct ToolTracker {
    active: HashSet<String>,
    owners: HashMap<String, ToolOwner>,
    seen: HashSet<String>,
    canary_count: usize,
    shell_permits: HashMap<String, ShellPermit>,
    /// A skill running in a forked subagent inside its `Skill` call.
    fork: Option<Fork>,
    /// The task of this turn's settled fork, whose late task frames (such as
    /// a kill notice after an interrupt) are ignored.
    settled_task: Option<String>,
}
/// A forked skill: native frames under `tool` belong to it until that `Skill`
/// call settles. Its `task` notification is evidence, not the end.
struct Fork {
    tool: String,
    task: String,
}
#[derive(Default)]
struct ReplyEchoes {
    pending: ByteQueue<Value>,
}
#[derive(Default)]
struct TurnState {
    active: Option<Turn>,
    assistant_text: Option<AssistantText>,
    completion: Option<Value>,
    input: InputEvidence,
    terminal_error: bool,
}
#[derive(Default)]
struct InputEvidence {
    // Independent evidence latches: initialization rejection does not by itself
    // prove that input was never queued or that native execution was not observed.
    enqueued: bool,
    rejected: bool,
    execution_observed: bool,
}
struct Turn {
    id: String,
    request: u64,
    acknowledged: bool,
    interrupt_sent: bool,
}
struct AssistantText {
    message: Option<String>,
    turn: String,
    text: String,
    blocks: usize,
}
struct ToolOwner {
    turn: String,
    name: String,
    allowed: bool,
    assistant_seen: bool,
    pre_seen: bool,
}
struct ShellPermit {
    token: String,
    command: String,
    permission_used: bool,
}
impl Codec {
    pub(crate) fn new(config: Arc<SessionConfig>, guard: ShellGuard) -> Self {
        Self {
            guard,
            steering_permit: config.steering,
            config,
            branch: None,
            initialization: None,
            callbacks: Callbacks::default(),
            tools: ToolTracker::default(),
            steering: steering::Steering::default(),
            frames: VecDeque::new(),
            echoes: ReplyEchoes::default(),
            turn: TurnState::default(),
            opened: false,
            verified: false,
            expected_method: crate::config::Method::Subscription,
            live: None,
            stream_message: None,
            deltas: Vec::new(),
        }
    }
    pub(crate) fn stream_live(&mut self, wake: Arc<crate::runtime::hub::Wake>) {
        self.live = Some(wake);
    }
    pub(crate) fn take_deltas(&mut self) -> Vec<crate::runtime::provider::live_tap::Delta> {
        std::mem::take(&mut self.deltas)
    }
    pub(crate) fn pop(&mut self) -> Option<Value> {
        self.frames.pop_front()
    }
    pub(crate) fn buffered(&self) -> bool {
        !self.frames.is_empty()
    }
    pub(crate) fn account_identity(&self) -> Option<&str> {
        self.initialization
            .as_ref()
            .and_then(|i| i.account.email.as_deref())
    }
    pub(crate) fn subscription(&self) -> bool {
        self.initialization
            .as_ref()
            .is_some_and(|i| i.account.subscription())
    }
    pub(crate) fn check_method(&mut self, method: crate::config::Method) -> bool {
        self.expected_method = method;
        self.selected_account()
    }
    fn selected_account(&self) -> bool {
        self.initialization
            .as_ref()
            .is_some_and(|i| i.account.matches(self.expected_method))
    }
    pub(crate) fn quiescent(&self) -> bool {
        self.guard.clean() && self.protocol_quiescent()
    }
    pub(crate) fn protocol_quiescent(&self) -> bool {
        !self.turn.input.execution_observed
            && self.turn.active.is_none()
            && self.tools.active.is_empty()
            && self.tools.fork.is_none()
            && self.callbacks.requests.is_empty()
            && self.callbacks.controls.is_empty()
    }
    pub(crate) fn preinput_rejected(&self) -> bool {
        self.turn.input.rejected && !self.turn.input.enqueued
    }
    pub(crate) fn invalidate_preinput_proof(&mut self) {
        self.turn.input.execution_observed = true;
    }
    pub(crate) fn terminal_error(&self) -> bool {
        self.turn.terminal_error
    }
    pub(crate) fn preinput_quiescent(&self) -> bool {
        !self.turn.input.enqueued
            && !self.turn.input.execution_observed
            && self.guard.sealed_clean()
            && self.turn.active.is_none()
            && self.tools.active.is_empty()
            && self.callbacks.requests.is_empty()
    }
    pub(crate) fn before_input(&self) -> bool {
        !self.turn.input.enqueued
    }
    pub(crate) fn settle(&mut self) {
        self.expire_steering(false, None);
        if self.guard.clean()
            && let Some(params) = self.turn.completion.take()
        {
            self.expire_steering(true, None);
            self.tools.owners.clear();
            self.tools.settled_task = None;
            self.turn.active = None;
            self.turn.assistant_text = None;
            self.notice(ServerMethod::TurnCompleted, params);
        }
    }
    fn reply(&mut self, id: u64, result: Value) {
        self.frames.push_back(emitted::reply(id, result));
    }
    fn notice(&mut self, method: ServerMethod, params: Value) {
        self.frames.push_back(emitted::notice(method, params));
    }
    fn check_session(&self, id: &str) -> Result<()> {
        if id != self.config.id {
            return Err(Error::session_mismatch("Claude session changed."));
        }
        Ok(())
    }
    fn check_model(&self, model: &str) -> Result<()> {
        let init = self.initialization.as_ref().ok_or_else(protocol)?;
        let matches = init.models.iter().any(|m| {
            (m.value == self.config.model || m.resolved == self.config.model)
                && m.resolved.trim_end_matches("[1m]") == model.trim_end_matches("[1m]")
        });
        if !matches {
            return Err(Error::model_mismatch("Claude selected a different model."));
        }
        Ok(())
    }
    fn active_id(&self) -> Result<String> {
        self.turn
            .active
            .as_ref()
            .filter(|t| t.acknowledged)
            .map(|t| t.id.clone())
            .ok_or_else(protocol)
    }
    fn acknowledge(
        &mut self,
        uuid: Option<String>,
        session: &str,
        parent: Option<String>,
    ) -> Result<()> {
        self.check_session(session)?;
        if parent.is_some() {
            return Err(protocol());
        }
        // Built-in tool-result messages are also user frames. Only the exact
        // UUID replay acknowledges our submitted input.
        let Some(turn) = self.turn.active.as_mut() else {
            return Err(protocol());
        };
        if uuid.as_deref() != Some(&turn.id) {
            return Ok(());
        }
        if !self.verified || turn.acknowledged {
            return Err(protocol());
        }
        let branch_input = turn.id.clone();
        if self.hold_branch_ack(&branch_input)? {
            return Ok(());
        }
        let turn = self.turn.active.as_mut().ok_or_else(protocol)?;
        turn.acknowledged = true;
        let (request, id) = (turn.request, turn.id.clone());
        self.reply(
            request,
            api::value(emitted::TurnResult {
                turn: emitted::Identity { id: &id },
            }),
        );
        self.notice(
            ServerMethod::TurnStarted,
            api::value(emitted::ThreadTurn {
                thread_id: &self.config.id,
                turn: emitted::Identity { id: &id },
            }),
        );
        Ok(())
    }
}
fn parse<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|cause| protocol().with_source(cause))
}
