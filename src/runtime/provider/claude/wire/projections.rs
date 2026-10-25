//! Lenient decision fields alongside opaque native user-content payloads.
use super::{
    ControlKind, FrameKind, McpMethod, McpServer, OriginKind, ResultSubtype, UserBlockKind,
};
use crate::runtime::provider::protocol::lenient;
use serde::{Deserialize, Deserializer, de::IgnoredAny};
use serde_json::Value;

#[derive(Default, Deserialize)]
pub(crate) struct PreinputMetadata {
    #[serde(default, rename = "type", deserialize_with = "lenient::optional")]
    pub kind: Option<FrameKind>,
    #[serde(default, deserialize_with = "lenient::defaulting")]
    request: MetadataRequest,
    #[serde(default, deserialize_with = "lenient::optional")]
    subtype: Option<ResultSubtype>,
    #[serde(default, deserialize_with = "lenient::optional")]
    is_error: Option<bool>,
    #[serde(default, deserialize_with = "lenient::optional")]
    num_turns: Option<u64>,
    #[serde(default)]
    user_message_uuid: Option<IgnoredAny>,
    #[serde(default)]
    terminal_reason: Option<IgnoredAny>,
    #[serde(default)]
    origin: Option<MetadataOrigin>,
}
#[derive(Default, Deserialize)]
struct MetadataRequest {
    #[serde(default, deserialize_with = "lenient::optional")]
    subtype: Option<ControlKind>,
    #[serde(default, deserialize_with = "lenient::optional")]
    server_name: Option<McpServer>,
    #[serde(default, deserialize_with = "lenient::defaulting")]
    message: MetadataMessage,
}
#[derive(Default, Deserialize)]
struct MetadataMessage {
    #[serde(default, deserialize_with = "lenient::optional")]
    method: Option<McpMethod>,
}
#[derive(Deserialize)]
struct MetadataOrigin(#[serde(deserialize_with = "lenient::defaulting")] OriginFields);
#[derive(Default, Deserialize)]
struct OriginFields {
    #[serde(default, deserialize_with = "lenient::optional")]
    kind: Option<OriginKind>,
}
impl PreinputMetadata {
    pub(crate) fn read(value: &Value) -> Self {
        lenient::defaulting(value).unwrap_or_default()
    }
    pub(crate) fn allowed(&self) -> bool {
        match self.kind {
            Some(FrameKind::ControlResponse) => true,
            Some(FrameKind::ControlRequest) => {
                self.request.subtype == Some(ControlKind::McpMessage)
                    && self.request.server_name == Some(McpServer::Hive)
                    && matches!(
                        self.request.message.method,
                        Some(
                            McpMethod::Initialize
                                | McpMethod::Ping
                                | McpMethod::ToolsList
                                | McpMethod::Initialized
                        )
                    )
            }
            Some(FrameKind::Result) => {
                self.subtype == Some(ResultSubtype::ErrorDuringExecution)
                    && self.is_error == Some(true)
                    && self.num_turns == Some(0)
                    && self.user_message_uuid.is_none()
                    && self.terminal_reason.is_none()
                    && self
                        .origin
                        .as_ref()
                        .is_none_or(|origin| origin.0.kind == Some(OriginKind::Human))
            }
            _ => false,
        }
    }
}

pub(crate) struct UserContent {
    pub blocks: Vec<UserBlock>,
}
impl<'de> Deserialize<'de> for UserContent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // A string, null or future content shape has no tool-result blocks.
        // Array entries each keep their own raw value, including malformed siblings.
        let blocks = match Value::deserialize(deserializer)? {
            Value::Array(blocks) => blocks.into_iter().map(UserBlock::new).collect(),
            _ => Vec::new(),
        };
        Ok(Self { blocks })
    }
}
#[derive(Default, Deserialize)]
struct UserFields {
    #[serde(default, rename = "type", deserialize_with = "lenient::optional")]
    kind: Option<UserBlockKind>,
    #[serde(default, deserialize_with = "lenient::optional")]
    tool_use_id: Option<String>,
}
pub(crate) struct UserBlock {
    fields: UserFields,
    pub raw: Value,
}
impl UserBlock {
    fn new(raw: Value) -> Self {
        let fields = lenient::defaulting(&raw).unwrap_or_default();
        Self { fields, raw }
    }
    pub(crate) fn tool_result(&self) -> bool {
        self.fields.kind == Some(UserBlockKind::ToolResult)
    }
    pub(crate) fn tool_use_id(&self) -> Option<&str> {
        self.fields.tool_use_id.as_deref()
    }
}
