//! Exact owned parameter shapes for requests sent to native provider transports.
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClientInfo {
    pub name: &'static str,
    pub title: &'static str,
    pub version: &'static str,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Capabilities {
    pub experimental_api: bool,
    // Noisy native notification families disabled during initialization. Live
    // streaming keeps only raw reasoning off; otherwise all four deltas stay off.
    pub opt_out_notification_methods: &'static [super::super::ServerMethod],
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Initialize {
    pub client_info: ClientInfo,
    pub capabilities: Capabilities,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountRead {
    pub refresh_token: bool,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelPage {
    pub include_hidden: bool,
    pub limit: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Value>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoadedThreadPage {
    pub limit: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}
#[derive(Clone, Serialize)]
pub(crate) struct Empty {}

#[derive(Clone, Serialize)]
pub(crate) struct EffortConfig {
    pub model_reasoning_effort: String,
    pub mcp_servers: crate::config::McpServers,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_verbosity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_auto_compact_token_limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_output_token_limit: Option<u64>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadBase {
    pub model: String,
    pub cwd: PathBuf,
    pub approval_policy: &'static str,
    pub sandbox: String,
    pub developer_instructions: String,
    pub config: Box<EffortConfig>,
    pub allow_provider_model_fallback: bool,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadStart {
    #[serde(flatten)]
    pub base: Option<ThreadBase>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic_tools: Option<Value>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadResume {
    #[serde(flatten)]
    pub base: Option<ThreadBase>,
    pub thread_id: String,
    pub exclude_turns: bool,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadFork {
    #[serde(flatten)]
    pub base: Option<ThreadBase>,
    pub thread_id: String,
    pub last_turn_id: String,
    pub exclude_turns: bool,
    pub ephemeral: bool,
}
#[derive(Clone, Serialize)]
pub(crate) struct InputText {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: String,
}
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum TurnInput {
    Text {
        text: String,
    },
    LocalImage {
        path: PathBuf,
        // Claude encoding is prepared off the writer; Codex sees only its native path.
        #[serde(skip)]
        claude: Option<EncodedImage>,
    },
}
#[derive(Clone, Serialize)]
pub(crate) struct EncodedImage {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub media_type: String,
    pub data: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TurnStart {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub input: Vec<TurnInput>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Thread {
    pub thread_id: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Steer {
    pub thread_id: String,
    pub expected_turn_id: String,
    pub input: Vec<InputText>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Interrupt {
    pub thread_id: String,
    pub turn_id: String,
}
