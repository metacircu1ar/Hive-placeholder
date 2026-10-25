//! Compaction and purge outcome responses.
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize)]
pub(crate) struct PurgeCounts {
    pub agents: usize,
    pub approvals: usize,
    pub messages: usize,
    pub attachments: usize,
    pub attachment_bytes: u64,
    pub deliveries: usize,
    pub call_results: usize,
    pub events: usize,
    pub event_retention: usize,
    pub redirects: usize,
    pub requests: usize,
    pub instruction_bindings: usize,
    pub instruction_snapshots: usize,
    pub artifacts: usize,
    pub join_members: usize,
    pub joins: usize,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PurgeBlocker {
    pub agent_id: String,
    pub reason: crate::runtime::domain::PurgeBlockerReason,
}
#[derive(Serialize)]
pub(crate) struct PurgeResponse {
    pub target: String,
    pub dry_run: bool,
    pub recursive: bool,
    pub selected_agents: usize,
    pub selected_preview: Vec<String>,
    pub blocked_agents: usize,
    pub blockers: Vec<PurgeBlocker>,
    pub counts: PurgeCounts,
    pub native_history: &'static str,
    pub retained: &'static str,
}
