//! Typed external MCP envelopes and projections; opaque tool data stays unchanged.
// JSON-RPC 2.0 reserved error codes; server errors cover phase/capacity rejection.
use super::McpMethod;
use crate::{calls::ObservationError, runtime::api::value as project};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{Error as DeserializeError, IgnoredAny},
};
use serde_json::{Error as JsonError, Map, Number, Value};
pub(super) const PARSE_ERROR: i32 = -32700;
pub(super) const INVALID_REQUEST: i32 = -32600;
pub(super) const METHOD_NOT_FOUND: i32 = -32601;
pub(super) const INVALID_PARAMS: i32 = -32602;
pub(super) const SERVER_ERROR: i32 = -32000;
pub(super) const INVALID_ENVELOPE: &str = "Invalid JSON-RPC envelope.";

#[derive(Clone, Serialize)]
#[serde(untagged)]
pub(super) enum RequestId {
    Text(String),
    Number(Number),
}
impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Value::deserialize(deserializer)? {
            Value::String(value) => Ok(Self::Text(value)),
            Value::Number(value) if value.is_i64() || value.is_u64() => Ok(Self::Number(value)),
            _ => Err(DeserializeError::custom("invalid JSON-RPC request ID")),
        }
    }
}
impl RequestId {
    #[expect(
        clippy::expect_used,
        reason = "RequestId contains only a string or a finite integer JSON number."
    )]
    pub(super) fn key(&self) -> String {
        serde_json::to_string(self).expect("MCP JSON serializes")
    }
}

#[derive(Deserialize)]
pub(super) struct Frame {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<RequestId>,
    pub method: McpMethod,
    #[serde(default)]
    pub params: Value,
}
#[derive(Deserialize)]
struct IdPresence {
    #[serde(default, rename = "id", deserialize_with = "is_null")]
    null: bool,
}
fn is_null<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    Option::<IgnoredAny>::deserialize(deserializer).map(|value| value.is_none())
}

pub(super) fn parse(bytes: &[u8]) -> Result<Frame, Message> {
    // JSON-RPC replies have fixed code/message fields and no internal error source.
    // Keep decoder diagnostics out of the wire; the logger retains only safe kinds/positions.
    // Value retains the existing duplicate-key last-wins rule before projection.
    let raw: Value = serde_json::from_slice(bytes).map_err(|cause| {
        crate::observability::observation_failed("mcp_json_parse", &cause);
        Message::error(None, PARSE_ERROR, "Invalid JSON.")
    })?;
    if !raw.is_object() {
        return Err(Message::error(
            None,
            INVALID_REQUEST,
            "JSON-RPC frames must be objects.",
        ));
    }
    // Explicit null wins over every other envelope defect; omission is a notification.
    if IdPresence::deserialize(&raw).is_ok_and(|id| id.null) {
        return Err(Message::error(
            None,
            INVALID_REQUEST,
            "Request ID must not be null.",
        ));
    }
    serde_json::from_value(raw).map_err(|cause| {
        crate::observability::observation_failed("mcp_envelope", &cause);
        Message::error(None, INVALID_REQUEST, INVALID_ENVELOPE)
    })
}

#[derive(Deserialize)]
struct Initialize {
    #[serde(rename = "protocolVersion")]
    protocol_version: String,
    #[serde(rename = "clientInfo")]
    _client_info: Map<String, Value>,
    #[serde(rename = "capabilities")]
    _capabilities: Map<String, Value>,
}
pub(super) fn initialize_protocol(params: Value) -> Option<&'static str> {
    if !params.is_object() {
        return None;
    }
    let parsed = serde_json::from_value::<Initialize>(params)
        .inspect_err(|cause| {
            crate::observability::observation_failed("mcp_initialize", cause);
        })
        .ok()?;
    hive_input::compatibility::select_mcp(
        &parsed.protocol_version,
        crate::compatibility::EXTERNAL_MCP,
    )
}
#[cfg(test)]
pub(super) fn valid_initialize(params: Value) -> bool {
    initialize_protocol(params).is_some()
}

#[derive(Deserialize)]
struct Cancellation {
    #[serde(default, rename = "requestId")]
    request_id: Option<RequestId>,
}
pub(super) fn cancellation(params: &Value) -> Option<RequestId> {
    // Other JSON kinds cannot match the key of an accepted request ID.
    params
        .is_object()
        .then(|| {
            crate::observability::optional(Cancellation::deserialize(params), "mcp_cancellation")?
                .request_id
        })
        .flatten()
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub(super) enum ProgressToken {
    Text(String),
    Number(Number),
}

// Fields below are declared in the sorted-key order of the previous Value wire
// projection. Direct serialization therefore preserves the exact response bytes.
#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum Message {
    Reply(Reply),
    Error(ErrorReply),
    Progress(Progress),
}
#[derive(Serialize)]
pub(super) struct Reply {
    id: RequestId,
    jsonrpc: &'static str,
    result: ResultBody,
}
#[derive(Serialize)]
pub(super) struct ErrorReply {
    error: RpcError,
    id: Option<RequestId>,
    jsonrpc: &'static str,
}
#[derive(Serialize)]
struct RpcError {
    code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
    message: String,
}
#[derive(Serialize)]
pub(super) struct Progress {
    jsonrpc: &'static str,
    method: &'static str,
    params: ProgressParams,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressParams {
    message: String,
    progress: u64,
    progress_token: ProgressToken,
}
#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum ResultBody {
    Initialize(Initialized),
    Empty(Empty),
    Catalog(Catalog),
    Tool(ToolResult),
}
#[derive(Serialize)]
pub(super) struct Empty {}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Initialized {
    capabilities: Capabilities,
    instructions: &'static str,
    protocol_version: &'static str,
    server_info: ServerInfo,
}
#[derive(Serialize)]
struct Capabilities {
    tools: Empty,
}
#[derive(Serialize)]
struct ServerInfo {
    icons: &'static [Icon; 1],
    name: &'static str,
    version: &'static str,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Icon {
    mime_type: &'static str,
    sizes: [&'static str; 1],
    src: &'static str,
}
#[derive(Serialize)]
pub(super) struct Catalog {
    tools: Vec<super::catalog::Definition>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ToolResult {
    content: [TextContent; 1],
    is_error: bool,
    structured_content: Value,
}
#[derive(Serialize)]
struct TextContent {
    text: String,
    #[serde(rename = "type")]
    kind: &'static str,
}

impl ResultBody {
    pub(super) fn initialize(protocol_version: &'static str) -> Self {
        Self::Initialize(Initialized {
            capabilities: Capabilities { tools: Empty {} },
            instructions: concat!(
                "External callers only. Long calls are resumed by message ID; timeouts ",
                "never cancel delivery. Reuse request_id for retries.",
            ),
            protocol_version,
            server_info: ServerInfo {
                name: "hive-external",
                version: env!("CARGO_PKG_VERSION"),
                icons: &[Icon {
                    src: include_str!("../../icons/generated/hive-64.data-uri"),
                    mime_type: "image/png",
                    sizes: ["64x64"],
                }],
            },
        })
    }
    pub(super) fn empty() -> Self {
        Self::Empty(Empty {})
    }
    pub(super) fn catalog(tools: Vec<super::catalog::Definition>) -> Self {
        Self::Catalog(Catalog { tools })
    }
    pub(super) fn tool(value: Value, error: bool) -> Self {
        Self::Tool(ToolResult {
            content: [TextContent {
                text: value.to_string(),
                kind: "text",
            }],
            is_error: error,
            structured_content: value,
        })
    }
}
impl Message {
    pub(super) fn reply(id: RequestId, result: ResultBody) -> Self {
        Self::Reply(Reply {
            id,
            jsonrpc: "2.0",
            result,
        })
    }
    pub(super) fn error(id: Option<RequestId>, code: i32, message: &str) -> Self {
        Self::Error(ErrorReply {
            error: RpcError {
                code,
                data: None,
                message: message.into(),
            },
            id,
            jsonrpc: "2.0",
        })
    }
    pub(super) fn input_error(id: RequestId, error: crate::error::Error) -> Self {
        Self::Error(ErrorReply {
            error: RpcError {
                code: INVALID_PARAMS,
                message: error.message().into(),
                data: Some(project(error)),
            },
            id: Some(id),
            jsonrpc: "2.0",
        })
    }
    pub(super) fn progress(token: ProgressToken, progress: u64, message: String) -> Self {
        Self::Progress(Progress {
            jsonrpc: "2.0",
            method: "notifications/progress",
            params: ProgressParams {
                message,
                progress,
                progress_token: token,
            },
        })
    }
}

#[derive(Deserialize)]
struct Metadata {
    #[serde(default, rename = "progressToken")]
    progress_token: ProgressField,
}
#[derive(Default)]
enum ProgressField {
    #[default]
    Missing,
    Value(ProgressToken),
}
impl<'de> Deserialize<'de> for ProgressField {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        ProgressToken::deserialize(deserializer).map(Self::Value)
    }
}
pub(super) fn progress_token(meta: Value) -> Result<Option<ProgressToken>, JsonError> {
    if !meta.is_object() {
        return Ok(None);
    }
    let Metadata { progress_token } = serde_json::from_value(meta)?;
    Ok(match progress_token {
        ProgressField::Missing => None,
        ProgressField::Value(token) => Some(token),
    })
}
#[derive(Serialize)]
struct ToolError {
    error: ObservationError,
}
pub(super) fn tool_error(error: ObservationError) -> Value {
    project(ToolError { error })
}

#[cfg(test)]
mod input_error_tests {
    #[test]
    fn tool_error_keeps_input_context_and_excludes_received() {
        let input = hive_input::InputError::new(
            hive_input::Code::OutOfRange,
            "Invalid page.",
            "/limit",
            "4 to 65536 bytes",
            vec!["hivectl output ARTIFACT_ID".into()],
        )
        .expect("valid fixture")
        .with_public_received("private-canary");
        let error = crate::error::Error::from(input);
        let expected = serde_json::to_value(&error).expect("wire error");
        let value = super::tool_error(error.into());
        assert_eq!(value["error"], expected);
        assert!(!value.to_string().contains("private-canary"));
    }
}
