//! Live-viewer replies: compact tree rows and one-snapshot attach.
use crate::runtime::{
    auth_fallback::{AuthDecision, WaitingReason},
    domain::{AgentState, TaskOutcome},
    live::{Cursor, TreeVersion},
};
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
pub(crate) struct TreeRow {
    pub id: String,
    pub parent_id: Option<String>,
    pub role: Option<String>,
    pub name: Option<String>,
    pub state: AgentState,
    pub task_outcome: TaskOutcome,
    pub waiting_reason: Option<WaitingReason>,
    pub auth: String,
    pub auth_group: Option<String>,
    pub auth_decision: Option<AuthDecision>,
    pub model: String,
    pub effort: String,
    pub sandbox: crate::config::Sandbox,
    pub ended_at: Option<u64>,
    pub children: u64,
    pub queued: u64,
    pub mail: u64,
    pub approval_id: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct TreeResponse {
    pub agents: Vec<TreeRow>,
    pub truncated: bool,
}

#[derive(Serialize)]
pub(crate) struct TreeReply {
    pub version: TreeVersion,
    #[serde(flatten)]
    pub tree: Value,
}

#[derive(Serialize)]
pub(crate) struct AttachResponse {
    /// The same agent view as `status`.
    pub agent: Value,
    /// The newest page or the page after `after`, as `read` returns it; or, with
    /// `before`, a history page whose `older` cursor continues backwards.
    pub events: Value,
    pub queued: u64,
    pub mail: u64,
    pub children: u64,
}

#[derive(Serialize)]
pub(crate) struct AttachReply {
    pub live: Cursor,
    #[serde(flatten)]
    pub snapshot: Value,
}
