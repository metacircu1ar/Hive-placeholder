//! Claude's nested MCP envelopes retain opaque native IDs and tool arguments.
use crate::runtime::api;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize)]
pub(crate) struct Empty {}
#[derive(Serialize)]
pub(crate) struct Payload<'a> {
    pub mcp_response: Response<'a>,
}
#[derive(Serialize)]
pub(crate) struct Response<'a> {
    pub jsonrpc: &'static str,
    pub id: Option<&'a Value>,
    #[serde(flatten)]
    pub outcome: Outcome,
}
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum Outcome {
    Success { result: ResultBody },
    Failure { error: Failure },
}
#[derive(Serialize)]
pub(crate) struct Failure {
    pub code: i32,
    pub message: &'static str,
}
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum ResultBody {
    Initialize(Initialize),
    Empty(Empty),
    Catalog(Catalog),
    Tool(ToolResult),
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Initialize {
    protocol_version: &'static str,
    capabilities: Capabilities,
    server_info: Server,
}
#[derive(Serialize)]
struct Capabilities {
    tools: Empty,
}
#[derive(Serialize)]
struct Server {
    name: &'static str,
    version: &'static str,
}
#[derive(Serialize)]
pub(crate) struct Catalog {
    pub tools: Vec<Definition>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Definition {
    name: String,
    description: String,
    input_schema: Value,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolResult {
    pub content: Vec<Text>,
    pub is_error: bool,
}
#[derive(Serialize)]
pub(crate) struct Text {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: String,
}
impl ResultBody {
    pub(crate) fn initialize(protocol_version: &'static str) -> Self {
        Self::Initialize(Initialize {
            protocol_version,
            capabilities: Capabilities { tools: Empty {} },
            server_info: Server {
                name: "hive",
                version: env!("CARGO_PKG_VERSION"),
            },
        })
    }
    pub(crate) const fn empty() -> Self {
        Self::Empty(Empty {})
    }
}
pub(crate) fn reply(control: &str, id: Option<&Value>, outcome: Outcome) -> Value {
    super::control::reply(
        control,
        Payload {
            mcp_response: Response {
                jsonrpc: "2.0",
                id,
                outcome,
            },
        },
    )
}
pub(crate) fn error(control: &str, id: Option<&Value>, code: i32, message: &'static str) -> Value {
    reply(
        control,
        id,
        Outcome::Failure {
            error: Failure { code, message },
        },
    )
}
pub(crate) fn empty() -> Value {
    api::value(Empty {})
}

/// Native Claude currently uses this MCP subset; negotiate its supported date.
pub(crate) fn initialize_protocol(params: &Value) -> Option<&'static str> {
    let requested = params.as_object()?.get("protocolVersion")?.as_str()?;
    hive_input::compatibility::select_mcp(requested, crate::compatibility::CLAUDE_MCP)
}
