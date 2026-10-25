//! Message-bound synchronous call outcomes and output references.
use crate::runtime::domain::{AgentState, MessageState};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CallOutcome {
    Pending,
    Held,
    Replied,
    Timeout,
    Uncertain,
    Cancelled,
    Failed,
    ObservationUnavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OutputSelection {
    FinalAnswer,
    LastAssistantMessage,
    NoText,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)] // Independent snapshot and output flags.
pub(crate) struct MessageResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_for_build: Option<crate::runtime::api::RequiredBuild>,

    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub not_before: Option<u64>,
    pub message_id: String,
    pub recipient: String,
    pub delivery_state: MessageState,
    #[serde(deserialize_with = "crate::runtime::domain::decode::string_enum")]
    pub outcome: CallOutcome,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub execution_id: Option<String>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub thread_id: Option<String>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub turn_id: Option<String>,
    /// Cursor immediately before the consuming turn's start in the event timeline.
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub events_after: Option<u64>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub output: Option<Value>,
    #[serde(deserialize_with = "crate::runtime::domain::decode::string_enum")]
    pub output_selection: OutputSelection,
    pub output_partial: bool,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub queued_ahead: Option<u64>,
    pub queue_count_exact: bool,
    pub active_input: bool,
    pub recipient_state: AgentState,
    pub paused: bool,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub delivery_error: Option<String>,
}
