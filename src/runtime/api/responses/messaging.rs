//! Message admission and delivery record responses.
use crate::runtime::domain::{Agent, DeliveryMode, MessageState};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SendResponse<R = String> {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_for_build: Option<crate::runtime::api::RequiredBuild>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_before: Option<u64>,
    pub message_id: String,
    pub delivery_state: MessageState,
    pub recipient: R,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::runtime::domain::decode::optional_object"
    )]
    pub steering_target: Option<SteeringTarget>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::runtime::domain::decode::optional_object"
    )]
    pub redirect_target: Option<RedirectTarget>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_field_names)] // Public identifiers follow the existing API names.
pub(crate) struct RedirectTarget {
    pub execution_id: String,
    pub message_id: String,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub turn_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SteeringTarget {
    pub execution_id: String,
    pub turn_id: String,
}
impl SteeringTarget {
    pub fn from_agent(agent: &Agent) -> Option<Self> {
        Some(Self {
            execution_id: agent.execution_id.clone()?,
            turn_id: agent.turn_id.clone()?,
        })
    }
}
#[derive(Serialize)]
pub(crate) struct MessageRecord {
    #[serde(skip)]
    pub has_attachments: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<crate::runtime::attachments::Attachment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_before: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule_integrity: Option<String>,
    pub seq: i64,
    pub id: String,
    pub sender: String,
    pub recipient: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<crate::runtime::domain::MessageKind>,
    pub state: MessageState,
    #[serde(skip_serializing_if = "DeliveryMode::is_next_turn")]
    pub mode: DeliveryMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steering_target: Option<SteeringTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_error: Option<String>,
}
#[derive(Serialize)]
pub(crate) struct MessagesPage {
    pub messages: Vec<Value>,
    pub next_before: Option<Value>,
}
