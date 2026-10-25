//! Typed translated frames; embedded native arguments, results and receipts stay opaque.
use crate::runtime::{api, domain::ProviderTurnStatus, provider::protocol::ServerMethod};
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
struct Reply<T> {
    id: u64,
    result: T,
}
#[derive(Serialize)]
struct Notice<T> {
    method: ServerMethod,
    params: T,
}
#[derive(Serialize)]
struct Rejection<'a> {
    id: u64,
    error: Failure<'a>,
}
#[derive(Serialize)]
struct Failure<'a> {
    code: i32,
    message: &'a str,
}
pub(crate) fn reply(id: u64, result: impl Serialize) -> Value {
    api::value(Reply { id, result })
}
pub(crate) fn notice(method: ServerMethod, params: impl Serialize) -> Value {
    api::value(Notice { method, params })
}
pub(crate) fn error(id: u64, code: i32, message: &str) -> Value {
    api::value(Rejection {
        id,
        error: Failure { code, message },
    })
}
#[derive(Serialize)]
pub(crate) struct Identity<'a> {
    pub id: &'a str,
}
#[derive(Serialize)]
pub(crate) struct TurnResult<T> {
    pub turn: T,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadTurn<'a, T> {
    pub thread_id: &'a str,
    pub turn: T,
}
#[derive(Serialize)]
pub(crate) struct TerminalTurn<'a> {
    pub id: &'a str,
    pub status: ProviderTurnStatus,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ItemParams<'a, T> {
    pub thread_id: &'a str,
    pub turn_id: &'a str,
    pub item: T,
}
#[derive(Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum AssistantItem {
    AgentMessage {
        // The native message ID keys live text to its durable message.
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        text: String,
        verified_model: bool,
    },
    DynamicToolCall {
        id: String,
        tool: String,
        arguments: Value,
    },
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Summary {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: String,
    pub verified_model: bool,
    pub aggregation: &'static str,
    pub native_message_id: Option<String>,
}
#[derive(Serialize)]
pub(crate) struct ToolResult<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub id: &'a str,
    pub result: &'a Value,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RateLimits<'a> {
    pub info: Value,
    pub thread_id: &'a str,
    pub turn_id: Option<&'a str>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderError<'a> {
    pub thread_id: &'a str,
    pub turn_id: Option<&'a str>,
    pub will_retry: bool,
    pub error: ErrorInfo<'a>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ErrorInfo<'a> {
    pub codex_error_info: &'a str,
}
#[derive(Serialize)]
pub(crate) struct User<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub uuid: &'a str,
    pub session_id: &'a str,
    pub message: UserText,
}
#[derive(Serialize)]
pub(crate) struct UserText {
    pub role: &'static str,
    pub content: UserContent,
}
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum UserContent {
    Text(String),
    Blocks(Vec<UserBlock>),
}
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum UserBlock {
    Text {
        text: String,
    },
    Image {
        source: crate::runtime::provider::protocol::outgoing::EncodedImage,
    },
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadSettings<'a> {
    pub thread: Identity<'a>,
    pub model: Option<&'a str>,
    pub reasoning_effort: &'a str,
    pub sandbox: &'a str,
    pub steering_disabled_reason: Option<crate::runtime::steering_canary::SteeringObservation>,
    pub steering_proof_error_code: Option<crate::error::ErrorCode>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Guidance {
    pub hook_specific_output: Context,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Context {
    pub hook_event_name: &'static str,
    pub additional_context: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SteeringDelivery<'a> {
    pub turn_id: &'a str,
    pub delivery_evidence: DeliveryEvidence<'a>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeliveryEvidence<'a> {
    pub kind: super::super::ReceiptMode,
    pub observation: &'a str,
    pub hook_reply: &'a Value,
}
