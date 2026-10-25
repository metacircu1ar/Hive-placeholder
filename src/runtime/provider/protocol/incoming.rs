//! Decode provider frames into typed views while preserving native payloads.
use super::{
    AccountKind, CoordinationTool, ItemKind, ServerMethod, UnsubscribeStatus,
    lenient::{defaulting, elements, optional},
};
use crate::error::Error;
use crate::runtime::domain::ProviderTurnStatus;
use crate::runtime::provider::protocol::codes;
use serde::Deserialize;
use serde_json::{Value, json};
use std::mem::size_of;
use std::sync::Arc;

#[derive(Clone, Default, Deserialize)]
struct TurnFields {
    #[serde(default, deserialize_with = "optional")]
    id: Option<String>,
    #[serde(default, deserialize_with = "optional")]
    status: Option<String>,
}

#[derive(Clone, Default, Deserialize)]
struct ItemFields {
    #[serde(default, rename = "type", deserialize_with = "optional")]
    kind: Option<ItemKind>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ParamsFields {
    #[serde(default, deserialize_with = "optional")]
    thread_id: Option<String>,
    #[serde(default, deserialize_with = "defaulting")]
    thread: TurnFields,
    #[serde(default, deserialize_with = "optional")]
    conversation_id: Option<String>,
    #[serde(default, deserialize_with = "optional")]
    turn_id: Option<String>,
    #[serde(default, deserialize_with = "defaulting")]
    turn: TurnFields,
    #[serde(default, deserialize_with = "defaulting")]
    item: ItemFields,
    #[serde(default, deserialize_with = "optional")]
    tool: Option<String>,
    #[serde(default, deserialize_with = "optional")]
    call_id: Option<String>,
}

#[derive(Clone, Default, Deserialize)]
struct ErrorFields {
    #[serde(default, deserialize_with = "optional")]
    code: Option<i64>,
    #[serde(default, deserialize_with = "optional")]
    message: Option<String>,
}

#[derive(Clone, Default, Deserialize)]
struct AccountFields {
    #[serde(default, deserialize_with = "optional")]
    email: Option<String>,
    #[serde(default, rename = "type", deserialize_with = "optional")]
    kind: Option<AccountKind>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(Debug))]
struct Effort {
    #[serde(default, deserialize_with = "optional")]
    reasoning_effort: Option<String>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(Debug))]
pub(crate) struct Model {
    #[serde(default, deserialize_with = "optional")]
    #[serde(rename = "model")]
    name: Option<String>,
    #[serde(default, deserialize_with = "optional")]
    #[serde(rename = "resolvedModel")]
    resolved: Option<String>,
    #[serde(default, deserialize_with = "elements")]
    supported_reasoning_efforts: Option<Vec<Effort>>,
}

pub(crate) fn validate_model(
    model: &str,
    effort: &str,
    catalog: &[Model],
) -> crate::error::Result<()> {
    validate_model_input(model, effort)?;
    let selected = catalog
        .iter()
        .find(|entry| entry.name() == Some(model))
        .ok_or(Error::unknown_model(
            "Requested model is absent from the provider catalog.",
        ))?;
    if !selected.supports_effort(effort) {
        return Err(Error::unsupported_effort(
            "Requested reasoning effort is absent from the model catalog.",
        ));
    }
    Ok(())
}

/// Reject malformed selections before native catalog work; catalog membership
/// still decides whether a syntactically valid selection is available.
pub(crate) fn validate_model_input(model: &str, effort: &str) -> crate::error::Result<()> {
    hive_input::validate::model(model).map_err(|cause| {
        Error::unknown_model("Requested model is absent from the provider catalog.")
            .with_source(cause)
    })?;
    hive_input::validate::effort(effort).map_err(|cause| {
        Error::unsupported_effort("Requested reasoning effort is absent from the model catalog.")
            .with_source(cause)
    })
}

impl Model {
    pub(crate) fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.name.as_ref().map_or(0, String::capacity)
            + self.resolved.as_ref().map_or(0, String::capacity)
            + self
                .supported_reasoning_efforts
                .as_ref()
                .map_or(0, |efforts| {
                    efforts.capacity() * size_of::<Effort>()
                        + efforts
                            .iter()
                            .map(|effort| {
                                effort.reasoning_effort.as_ref().map_or(0, String::capacity)
                            })
                            .sum::<usize>()
                })
    }
    #[cfg(test)]
    pub(crate) fn parse(raw: &Value) -> Self {
        defaulting(raw).unwrap_or_default()
    }
    pub(crate) fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
    pub(crate) fn resolved_name(&self) -> Option<&str> {
        self.resolved.as_deref()
    }
    pub(crate) fn supports_effort(&self, effort: &str) -> bool {
        self.supported_reasoning_efforts
            .as_ref()
            .is_some_and(|efforts| {
                efforts
                    .iter()
                    .any(|entry| entry.reasoning_effort.as_deref() == Some(effort))
            })
    }
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResponseFields {
    #[serde(default, deserialize_with = "optional")]
    steering_disabled_reason: Option<String>,
    #[serde(default, deserialize_with = "optional")]
    steering_proof_error_code: Option<String>,
    #[serde(default, deserialize_with = "optional")]
    turn_id: Option<String>,
    #[serde(default, deserialize_with = "defaulting")]
    thread: TurnFields,
    #[serde(default, deserialize_with = "defaulting")]
    turn: TurnFields,
    #[serde(default, deserialize_with = "optional")]
    model: Option<String>,
    #[serde(default, deserialize_with = "optional")]
    reasoning_effort: Option<String>,
    #[serde(default, deserialize_with = "optional")]
    status: Option<String>,
    #[serde(default, deserialize_with = "optional")]
    requires_openai_auth: Option<bool>,
    #[serde(default, deserialize_with = "defaulting")]
    account: AccountFields,
}

#[derive(Clone, Default, Deserialize)]
struct FrameFields {
    #[serde(default, deserialize_with = "optional")]
    id: Option<u64>,
    #[serde(default, deserialize_with = "optional")]
    method: Option<ServerMethod>,
    #[serde(default, deserialize_with = "defaulting")]
    params: ParamsFields,
    #[serde(default, deserialize_with = "defaulting")]
    result: ResponseFields,
    #[serde(default, deserialize_with = "defaulting")]
    error: ErrorFields,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    Response,
    Request,
    Notification,
}

#[derive(Clone)]
pub(crate) struct IncomingFrame {
    raw: Value,
    // Cloned frames share parsed decisions and own their raw payload independently.
    fields: Arc<FrameFields>,
    kind: FrameKind,
    // Provider read order, so live deltas publish before later deferred frames.
    seq: u64,
}

impl IncomingFrame {
    pub(crate) fn parse(raw: Value) -> Self {
        let fields = defaulting(&raw).unwrap_or_default();
        let kind = if raw.get("method").is_none() {
            FrameKind::Response
        } else if raw.get("id").is_some() {
            FrameKind::Request
        } else {
            FrameKind::Notification
        };
        Self {
            raw,
            fields: Arc::new(fields),
            kind,
            seq: 0,
        }
    }
    pub(crate) fn with_seq(mut self, seq: u64) -> Self {
        self.seq = seq;
        self
    }
    pub(crate) fn seq(&self) -> u64 {
        self.seq
    }
    #[cfg(test)]
    pub(crate) fn raw(&self) -> &Value {
        &self.raw
    }
    #[cfg(test)]
    pub(crate) fn into_raw(self) -> Value {
        self.raw
    }
    pub(crate) fn encoded_len(&self) -> usize {
        self.raw.to_string().len()
    }
    pub(crate) fn is_response(&self) -> bool {
        self.kind == FrameKind::Response
    }
    pub(crate) fn is_request(&self) -> bool {
        self.kind == FrameKind::Request
    }
    pub(crate) fn has_id(&self) -> bool {
        self.raw.get("id").is_some()
    }
    pub(crate) fn id(&self) -> Option<u64> {
        self.fields.id
    }
    pub(crate) fn raw_id(&self) -> &Value {
        &self.raw["id"]
    }
    pub(crate) fn matches_response(&self, id: u64) -> bool {
        self.is_response() && self.raw["id"] == id
    }
    pub(crate) fn method(&self) -> Option<&ServerMethod> {
        self.fields.method.as_ref()
    }
    pub(crate) fn params(&self) -> ParamsRef<'_> {
        ParamsRef {
            raw: &self.raw["params"],
            fields: &self.fields.params,
        }
    }
    pub(crate) fn has_result(&self) -> bool {
        self.raw.get("result").is_some()
    }
    pub(crate) fn has_error(&self) -> bool {
        self.raw.get("error").is_some()
    }
    pub(crate) fn error_data(&self) -> &Value {
        &self.raw["error"]["data"]
    }
    pub(crate) fn valid_error(&self) -> bool {
        self.fields.error.code.is_some() && self.fields.error.message.is_some()
    }
    pub(crate) fn fork_error(&self, cutoff: Option<&str>) -> Error {
        let error =
            Error::provider_request_failed("Provider rejected native conversation branching.");
        let message = self.fields.error.message.as_deref().unwrap_or_default();
        if self.fields.error.code == Some(i64::from(codes::INVALID_REQUEST))
            && cutoff.is_some_and(|turn| {
                message == format!("lastTurnId '{turn}' identifies an in-progress turn")
                    || message == format!("lastTurnId '{turn}' was not found in the source thread")
                    || message == format!("turn not found: {turn}")
                    || message == format!("turn {turn} does not have persisted rollout positions")
            })
        {
            return error.before_submission();
        }
        error
    }
    /// Codex 0.154 has no structured stale-turn discriminator. Recognize only
    /// its exact validation errors, keeping all other errors conservative.
    pub(crate) fn steering_error(&self, expected: Option<&str>) -> Error {
        let code = self.fields.error.code;
        let message = self.fields.error.message.as_deref().unwrap_or_default();
        if code == Some(i64::from(codes::METHOD_NOT_FOUND)) {
            return Error::unsupported_operation("Provider does not support steering.");
        }
        if code == Some(i64::from(codes::INVALID_REQUEST)) {
            if message == "no active turn to steer" {
                return Error::no_active_turn("The steering target turn has ended.");
            }
            if expected.is_some_and(|turn| {
                message
                    .strip_prefix(&format!("expected active turn id `{turn}` but found `"))
                    .is_some_and(|tail| tail.ends_with('`') && tail.len() > 1)
            }) {
                return Error::stale_turn("The steering target is no longer the active turn.");
            }
        }
        if code == Some(i64::from(codes::INVALID_REQUEST))
            || code == Some(i64::from(codes::INVALID_PARAMS))
        {
            return Error::provider_request_failed("Provider rejected steering input.");
        }
        // Codex can enqueue guidance before returning an internal error.
        Error::steering_uncertain(
            "Provider could not confirm steering delivery; it will not be replayed.",
        )
    }
    pub(crate) fn result(&self) -> Option<Response> {
        self.raw.get("result").map(|raw| Response {
            raw: raw.clone(),
            fields: Arc::clone(&self.fields),
        })
    }
    // Preserve the whole native status value when synthesizing an early lifecycle frame.
    pub(crate) fn early(method: &ServerMethod, thread: &str, turn: &str, status: &Value) -> Self {
        Self::parse(json!({"method":method,"params":{"threadId":thread,
            "turn":{"id":turn,"status":status}}}))
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ParamsRef<'a> {
    raw: &'a Value,
    fields: &'a ParamsFields,
}

impl<'a> ParamsRef<'a> {
    pub(crate) fn raw(self) -> &'a Value {
        self.raw
    }
    pub(crate) fn thread_id(self) -> Option<&'a str> {
        self.fields.thread_id.as_deref()
    }
    pub(crate) fn thread_key(self) -> Option<&'a str> {
        self.fields
            .thread_id
            .as_deref()
            .or(self.fields.thread.id.as_deref())
            .or(self.fields.conversation_id.as_deref())
    }
    pub(crate) fn turn_id(self) -> Option<&'a str> {
        self.fields.turn_id.as_deref()
    }
    pub(crate) fn nested_turn_id(self) -> Option<&'a str> {
        self.fields.turn.id.as_deref()
    }
    pub(crate) fn turn_status(self) -> Option<ProviderTurnStatus> {
        self.fields
            .turn
            .status
            .as_deref()
            .and_then(ProviderTurnStatus::parse)
    }
    // Original status text is preserved in evidence, including unknown provider values.
    pub(crate) fn status(self) -> Option<&'a str> {
        self.fields.turn.status.as_deref()
    }
    pub(crate) fn item_kind(self) -> Option<ItemKind> {
        self.fields.item.kind
    }
    pub(crate) fn tool(self) -> Option<CoordinationTool> {
        self.fields.tool.as_deref().map(CoordinationTool::parse)
    }
    pub(crate) fn call_id(self) -> Option<&'a str> {
        self.fields.call_id.as_deref()
    }
    // These values are evidence/tool payloads, not reconstructed projections.
    pub(crate) fn raw_turn_id(self) -> &'a Value {
        &self.raw["turnId"]
    }
    pub(crate) fn raw_status(self) -> &'a Value {
        &self.raw["turn"]["status"]
    }
    // Input<String> validates this original value at the tool-dispatch boundary.
    pub(crate) fn raw_tool(self) -> &'a Value {
        &self.raw["tool"]
    }
    pub(crate) fn arguments(self) -> &'a Value {
        &self.raw["arguments"]
    }
    pub(crate) fn item(self) -> &'a Value {
        &self.raw["item"]
    }
    pub(crate) fn text(self) -> &'a Value {
        &self.raw["item"]["text"]
    }
    pub(crate) fn will_retry(self) -> &'a Value {
        &self.raw["willRetry"]
    }
    pub(crate) fn to_owned(self) -> OwnedParams {
        OwnedParams {
            raw: self.raw.clone(),
            fields: self.fields.clone(),
        }
    }
}

pub(crate) struct OwnedParams {
    raw: Value,
    fields: ParamsFields,
}
impl OwnedParams {
    pub(crate) fn view(&self) -> ParamsRef<'_> {
        ParamsRef {
            raw: &self.raw,
            fields: &self.fields,
        }
    }
}

#[derive(Clone)]
pub(crate) struct Response {
    raw: Value,
    fields: Arc<FrameFields>,
}
impl Response {
    #[cfg(test)]
    pub(crate) fn parse(raw: Value) -> Self {
        let fields = Arc::new(FrameFields {
            result: defaulting(&raw).unwrap_or_default(),
            ..FrameFields::default()
        });
        Self { raw, fields }
    }
    pub(crate) fn into_raw(self) -> Value {
        self.raw
    }
    pub(crate) fn thread_id(&self) -> Option<&str> {
        self.fields.result.thread.id.as_deref()
    }
    pub(crate) fn turn_id(&self) -> Option<&str> {
        self.fields.result.turn.id.as_deref()
    }
    pub(crate) fn steering_disabled_reason(&self) -> Option<&str> {
        self.fields.result.steering_disabled_reason.as_deref()
    }
    pub(crate) fn steering_proof_error_code(&self) -> Option<&str> {
        self.fields.result.steering_proof_error_code.as_deref()
    }
    pub(crate) fn steered_turn_id(&self) -> Option<&str> {
        self.fields.result.turn_id.as_deref()
    }
    pub(crate) fn model(&self) -> Option<&str> {
        self.fields.result.model.as_deref()
    }
    pub(crate) fn effort(&self) -> Option<&str> {
        self.fields.result.reasoning_effort.as_deref()
    }
    pub(crate) fn status(&self) -> Option<UnsubscribeStatus> {
        self.fields
            .result
            .status
            .as_deref()
            .map(UnsubscribeStatus::parse)
    }
    // Only catalog consumers project entries; unrelated response arrays stay opaque.
    pub(crate) fn catalog(&self) -> Option<Vec<Model>> {
        elements(&self.raw["data"]).unwrap_or_default()
    }
    // Terminal cleanup needs array presence and emptiness, not model projections.
    pub(crate) fn data_is_empty(&self) -> Option<bool> {
        self.raw["data"].as_array().map(Vec::is_empty)
    }
    pub(crate) fn terminals_empty(&self) -> crate::error::Result<bool> {
        Ok(self
            .data_is_empty()
            .ok_or_else(super::super::error::protocol)?
            && self.cursor().is_null())
    }
    /// Parse one exact `thread/loaded/list` page. The cleanup caller requests the
    /// maximum supported resident-session count and rejects pagination so absence
    /// is established from one provider snapshot rather than a shifting scan.
    pub(crate) fn loaded_threads(&self) -> crate::error::Result<(Vec<&str>, Option<&str>)> {
        const ID_BYTES: usize = 1024;
        let data = self
            .raw
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(super::super::error::protocol)?;
        let mut threads = Vec::with_capacity(data.len());
        for value in data {
            let id = value
                .as_str()
                .filter(|id| !id.is_empty() && id.len() <= ID_BYTES)
                .ok_or_else(super::super::error::protocol)?;
            threads.push(id);
        }
        let cursor = match self.raw.get("nextCursor") {
            None | Some(Value::Null) => None,
            Some(Value::String(cursor)) if !cursor.is_empty() && cursor.len() <= ID_BYTES => {
                Some(cursor.as_str())
            }
            Some(_) => return Err(super::super::error::protocol()),
        };
        Ok((threads, cursor))
    }
    pub(crate) fn account_identity(&self) -> Option<&str> {
        self.fields.result.account.email.as_deref()
    }
    pub(crate) fn subscription_account(&self) -> bool {
        self.fields.result.requires_openai_auth == Some(true)
            && self.fields.result.account.kind == Some(AccountKind::Chatgpt)
    }
    pub(crate) fn api_account(&self) -> bool {
        self.fields.result.requires_openai_auth == Some(true)
            && self.fields.result.account.kind == Some(AccountKind::ApiKey)
    }
    // Pagination forwards nonnull cursors of every JSON type without narrowing them.
    pub(crate) fn cursor(&self) -> &Value {
        &self.raw["nextCursor"]
    }
    // Preserve deferred Input<String> validation of the original acknowledgement ID.
    pub(crate) fn raw_thread_id(&self) -> &Value {
        &self.raw["thread"]["id"]
    }
    // Preserve deferred Input<String> validation of the original acknowledgement ID.
    pub(crate) fn raw_turn_id(&self) -> &Value {
        &self.raw["turn"]["id"]
    }
    pub(crate) fn raw_model(&self) -> &Value {
        &self.raw["model"]
    }
    pub(crate) fn raw_effort(&self) -> &Value {
        &self.raw["reasoningEffort"]
    }
    pub(crate) fn sandbox(&self) -> &Value {
        &self.raw["sandbox"]
    }
}

#[cfg(test)]
mod tests;
