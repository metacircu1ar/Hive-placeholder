//! Native control envelopes are owned here; hook payloads remain raw for exact echo matching.
use crate::runtime::api;
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
struct Envelope<T> {
    #[serde(rename = "type")]
    kind: &'static str,
    response: T,
}
#[derive(Serialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
enum Response<'a, T> {
    Success {
        request_id: &'a str,
        response: T,
    },
    Error {
        request_id: &'a str,
        error: &'static str,
    },
}
pub(crate) fn reply<T: Serialize>(id: &str, payload: T) -> Value {
    api::value(Envelope {
        kind: "control_response",
        response: Response::Success {
            request_id: id,
            response: payload,
        },
    })
}
pub(crate) fn unsupported(id: &str) -> Value {
    api::value(Envelope {
        kind: "control_response",
        response: Response::<super::mcp::Empty>::Error {
            request_id: id,
            error: "Unsupported control request.",
        },
    })
}
#[derive(Serialize)]
#[serde(tag = "behavior", rename_all = "snake_case")]
pub(crate) enum Permission {
    Allow {
        #[serde(rename = "updatedInput")]
        input: Value,
    },
    Deny {
        message: &'static str,
    },
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HookOutput {
    pub hook_specific_output: HookDecision,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HookDecision {
    pub hook_event_name: &'static str,
    pub permission_decision: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<Value>,
    pub permission_decision_reason: &'static str,
}
#[derive(Serialize)]
pub(crate) struct OutboundControl<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub request_id: &'a str,
    pub request: NativeRequest,
}
#[derive(Serialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub(crate) enum NativeRequest {
    Initialize { hooks: Hooks },
    GetSettings,
    Interrupt { cancel_queued: bool },
}
#[derive(Default, Serialize)]
pub(crate) struct Hooks {
    #[serde(rename = "PreToolUse")]
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<[Hook; 1]>,
    #[serde(rename = "PostToolUse")]
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<[Hook; 1]>,
    #[serde(rename = "PostToolUseFailure")]
    #[serde(skip_serializing_if = "Option::is_none")]
    failed: Option<[Hook; 1]>,
}
#[derive(Serialize)]
struct Hook {
    #[serde(rename = "hookCallbackIds")]
    callbacks: [&'static str; 1],
}
impl NativeRequest {
    pub(crate) fn discovery() -> Self {
        Self::Initialize {
            hooks: Hooks::default(),
        }
    }
    pub(crate) const fn initialize() -> Self {
        Self::Initialize {
            hooks: Hooks {
                before: Some([Hook {
                    callbacks: ["hive-guard"],
                }]),
                after: Some([Hook {
                    callbacks: ["hive-finished"],
                }]),
                failed: Some([Hook {
                    callbacks: ["hive-finished"],
                }]),
            },
        }
    }
}
#[derive(Serialize)]
pub(crate) struct ToolCall<'a> {
    pub id: u64,
    pub method: crate::runtime::provider::protocol::ServerMethod,
    pub params: ToolArguments<'a>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolArguments<'a> {
    pub thread_id: &'a str,
    pub turn_id: &'a str,
    pub call_id: String,
    pub tool: String,
    pub arguments: Value,
}
