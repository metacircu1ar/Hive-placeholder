//! Advisory human questions. These records do not confer execution permissions.
use crate::runtime::domain::domain_enum;
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod formats;
mod schema;
pub(crate) use schema::{valid_response, valid_schema};

// Apply the same field cap to form structure and the human-facing prompt.
pub(crate) const SCHEMA_BYTES: usize = 16 * 1024;
pub(crate) const MESSAGE_BYTES: usize = SCHEMA_BYTES;
// Titles stay short; answers reserve room for the enclosing control message.
pub(crate) const TITLE_BYTES: usize = 256;
pub(crate) const RESPONSE_BYTES: usize = 60 * 1024;
pub(crate) const BODY_BYTES: usize = crate::limits::CONTROL_INPUT_BYTES;
const _: () = assert!(RESPONSE_BYTES < BODY_BYTES);

domain_enum!(ApprovalAction {
    Accept => "accept",
    Decline => "decline",
    Cancel => "cancel",
});
domain_enum!(ApprovalState {
    Pending => "pending",
    Answered => "answered",
    Superseded => "superseded",
    Withdrawn => "withdrawn",
});

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ApprovalCursor {
    pub created_at: u64,
    pub id: String,
}

impl<'de> Deserialize<'de> for ApprovalCursor {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            created_at: u64,
            id: String,
        }
        let Fields { created_at, id } = crate::runtime::domain::decode::object(deserializer)?;
        Ok(Self { created_at, id })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Approval {
    pub id: String,
    pub agent_id: String,
    pub execution: String,
    pub turn: String,
    pub title: Option<String>,
    pub message: String,
    pub schema: Option<Value>,
    pub destructive: bool,
    pub state: ApprovalState,
    pub action: Option<ApprovalAction>,
    pub content: Option<Value>,
    pub reason: Option<String>,
    pub created_at: u64,
    pub closed_at: Option<u64>,
    pub response_message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApprovalSummary {
    pub id: String,
    pub agent_id: String,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub role: Option<String>,
    /// The asking agent's display name.
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub agent_name: Option<String>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub title: Option<String>,
    pub destructive: bool,
    pub state: ApprovalState,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub action: Option<ApprovalAction>,
    pub created_at: u64,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub closed_at: Option<u64>,
    pub age_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApprovalsPage {
    pub approvals: Vec<ApprovalSummary>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub next_after: Option<ApprovalCursor>,
}

#[derive(Serialize)]
pub(crate) struct ApprovalResponse {
    pub approval: Approval,
}

#[derive(Serialize)]
pub(crate) struct AskResponse {
    pub approval_id: String,
    pub state: ApprovalState,
}

#[derive(Serialize)]
pub(crate) struct RespondResponse {
    pub approval_id: String,
    pub state: ApprovalState,
    pub action: ApprovalAction,
    pub message_id: String,
    pub recipient: String,
}
