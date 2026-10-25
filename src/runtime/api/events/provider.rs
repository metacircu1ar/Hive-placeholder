//! Provider and execution event envelopes; native evidence fields stay borrowed and opaque.
use crate::{
    error::Error,
    runtime::{
        domain::{MessageState, TaskOutcome},
        provider::Shutdown,
    },
};
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
pub(crate) struct ExecutionFailure<'a> {
    pub error: &'a Error,
    pub turn_id: Option<&'a str>,
}
#[derive(Serialize)]
pub(crate) struct ErrorEvent<'a> {
    pub error: &'a Error,
}
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum ExecutionStopped<'a> {
    Unsubmitted {
        reason: &'static str,
        error: &'a Error,
    },
    Outcome {
        task_outcome: TaskOutcome,
    },
}
#[derive(Serialize)]
pub(crate) struct ExecutionStarting<'a> {
    pub model: &'a str,
    pub effort: &'a str,
    pub options_revision: u64,
}
#[derive(Serialize)]
pub(crate) struct ProcessStart {
    pub pid: u32,
    pub spawn_before: u64,
    pub spawn_after: u64,
}
#[derive(Serialize)]
pub(crate) struct ProviderInitialized {
    pub compatibility: Option<crate::provider_support::Observation>,
    pub reply: Value,
}
#[derive(Serialize)]
pub(crate) struct ProviderStarted<'a> {
    pub provider_id: &'a str,
    pub pgid: u32,
    #[serde(flatten)]
    pub process: ProcessStart,
}
#[derive(Serialize)]
pub(crate) struct ProcessShutdown<'a> {
    pub shutdown: &'a Shutdown,
    pub error_code: Option<&'a str>,
}
#[derive(Serialize)]
pub(crate) struct ProviderNotice {
    pub scope: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<&'static str>,
}
#[derive(Serialize)]
pub(crate) struct ThreadBound<'a> {
    pub thread_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub late_ack: Option<bool>,
}
#[derive(Serialize)]
pub(crate) struct SessionReady<'a> {
    pub thread_id: &'a str,
    pub model: &'a Value,
    pub effort: &'a Value,
    pub options_revision: u64,
    pub selection_source: &'a str,
    pub sandbox: &'a Value,
    pub resumed: bool,
    #[serde(flatten)]
    pub steering: Option<SteeringDisabled<'a>>,
}
#[derive(Serialize)]
pub(crate) struct SteeringDisabled<'a> {
    pub steering_proof_error_code: &'a str,
    pub steering_disabled_reason: Option<&'a str>,
}
#[derive(Serialize)]
pub(crate) struct TerminalCleanupObserved<'a, T> {
    pub turn_id: &'a str,
    pub scope: &'static str,
    pub remaining: u64,
    pub next_cursor: Option<()>,
    pub observation: T,
}
#[derive(Serialize)]
pub(crate) struct TurnFinished<'a> {
    pub turn_id: Option<&'a str>,
    pub status: &'a str,
    pub message_id: &'a str,
}
#[derive(Serialize)]
pub(crate) struct ProviderError<'a> {
    pub turn_id: &'a Value,
    pub will_retry: &'a Value,
}
#[derive(Serialize)]
pub(crate) struct ToolCompleted<'a> {
    pub turn_id: &'a Value,
    pub item: &'a Value,
}
#[derive(Serialize)]
pub(crate) struct AssistantOutput<'a> {
    pub turn_id: &'a Value,
    /// Native item key shared with live text: Codex item ID or Claude message ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<&'a Value>,
    pub text: &'a Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<&'a Value>,
    #[serde(flatten)]
    pub aggregate: Option<OutputAggregation<'a>>,
}
#[derive(Serialize)]
pub(crate) struct OutputAggregation<'a> {
    pub aggregation: &'a Value,
    pub native_message_id: &'a Value,
}
#[derive(Serialize)]
pub(crate) struct SteeringOutcome<'a> {
    pub message_id: &'a str,
    pub turn_id: &'a str,
    pub state: MessageState,
    pub error: Option<&'a str>,
    pub receipt: Option<&'a Value>,
}

#[cfg(test)]
mod tests;
