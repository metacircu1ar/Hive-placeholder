//! Typed transport envelopes and responses with opaque external request identities.
use super::{ClientMethod, OutgoingRequest, params::InputText};
use crate::runtime::provider::protocol::codes;
use serde::{Serialize, Serializer};
use serde_json::Value;

pub(crate) enum OutgoingFrame {
    Request { id: u64, request: OutgoingRequest },
    Initialized { method: ClientMethod },
    Response(ToolResponse),
}
impl Serialize for OutgoingFrame {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Request<'a> {
            id: u64,
            method: ClientMethod,
            params: &'a OutgoingRequest,
        }
        #[derive(Serialize)]
        struct Notification {
            method: ClientMethod,
        }
        match self {
            Self::Request { id, request } => Request {
                id: *id,
                method: request.method(),
                params: request,
            }
            .serialize(serializer),
            Self::Initialized { method } => Notification { method: *method }.serialize(serializer),
            Self::Response(response) => response.serialize(serializer),
        }
    }
}
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum ToolResponse {
    Success { id: Value, result: ToolResult },
    Failure { id: Value, error: ResponseError },
}
impl ToolResponse {
    pub(crate) fn id(&self) -> &Value {
        match self {
            Self::Success { id, .. } | Self::Failure { id, .. } => id,
        }
    }
}
#[derive(Serialize)]
pub(crate) struct ResponseError {
    pub code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolResult {
    pub content_items: Vec<InputText>,
    pub success: bool,
}
pub(crate) fn request(id: u64, request: OutgoingRequest) -> OutgoingFrame {
    OutgoingFrame::Request { id, request }
}
pub(crate) fn initialized() -> OutgoingFrame {
    OutgoingFrame::Initialized {
        method: ClientMethod::Initialized,
    }
}
pub(crate) fn unsupported(id: &Value, message: &str) -> OutgoingFrame {
    OutgoingFrame::Response(ToolResponse::Failure {
        id: id.clone(),
        error: ResponseError {
            code: codes::METHOD_NOT_FOUND,
            message: Some(message.into()),
        },
    })
}
pub(crate) fn tool_reply(id: &Value, result: &Value, success: bool) -> OutgoingFrame {
    OutgoingFrame::Response(ToolResponse::Success {
        id: id.clone(),
        result: ToolResult {
            content_items: vec![InputText {
                kind: "inputText",
                text: result.to_string(),
            }],
            success,
        },
    })
}
