//! Typed garbage collection plans and outcome records.
use super::super::GcCursor;
use serde::{Deserialize, Serialize};

pub(crate) const GC_PREVIEW: usize = 20;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GcStats {
    pub examined_agents: u64,
    pub eligible_agents: u64,
    pub deleted_agents: u64,
    pub blocked_agents: u64,
    pub messages: u64,
    pub attachments: u64,
    pub attachment_bytes: u64,
    pub approvals: u64,
    pub events: u64,
    pub artifacts: u64,
    pub indexed_output_bytes: u64,
    pub work_rows: u64,
    pub largest_agent_rows: u64,
}
impl GcStats {
    pub fn add(&mut self, page: &Self) {
        self.examined_agents += page.examined_agents;
        self.eligible_agents += page.eligible_agents;
        self.deleted_agents += page.deleted_agents;
        self.blocked_agents += page.blocked_agents;
        self.messages += page.messages;
        self.attachments += page.attachments;
        self.attachment_bytes += page.attachment_bytes;
        self.approvals += page.approvals;
        self.events += page.events;
        self.artifacts += page.artifacts;
        self.indexed_output_bytes += page.indexed_output_bytes;
        self.work_rows += page.work_rows;
        self.largest_agent_rows = self.largest_agent_rows.max(page.largest_agent_rows);
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GcPage {
    /// The control socket adds this receipt for mutating GC requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub before: u64,
    pub dry_run: bool,
    #[serde(deserialize_with = "crate::runtime::domain::decode::object")]
    pub stats: GcStats,
    pub selected_preview: Vec<String>,
    #[serde(deserialize_with = "super::read::objects")]
    pub blockers: Vec<super::PurgeBlocker>,
    #[serde(deserialize_with = "crate::runtime::domain::decode::optional_object")]
    pub next_after: Option<GcCursor>,
    pub cleanup_pending: bool,
}
