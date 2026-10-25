//! Only fields used for decisions are projected. Tool arguments remain opaque.
use serde::Deserialize;
use serde_json::Value;

pub(super) mod catalog;
pub(super) mod control;
pub(super) mod emitted;
pub(super) mod mcp;
mod projections;
pub(super) mod settings;
pub(super) use projections::{PreinputMetadata, UserContent};

use super::super::protocol::wire_enum;
wire_enum!(FrameKind {
    ControlResponse => "control_response", ControlRequest => "control_request",
    Result => "result",
});
wire_enum!(ControlKind { McpMessage => "mcp_message" });
wire_enum!(ControlSubtype { Success => "success" });
wire_enum!(ResultSubtype {
    Success => "success", ErrorDuringExecution => "error_during_execution",
});
wire_enum!(TerminalReason {
    Completed => "completed", BackgroundRequested => "background_requested",
    AbortedStreaming => "aborted_streaming", AbortedTools => "aborted_tools",
});
wire_enum!(SystemSubtype {
    Init => "init", Status => "status", TaskStarted => "task_started",
    TaskNotification => "task_notification",
    TaskProgress => "task_progress", TaskUpdated => "task_updated",
    BackgroundTasksChanged => "background_tasks_changed",
});
wire_enum!(OriginKind { Human => "human" });
wire_enum!(HookEvent {
    PreToolUse => "PreToolUse", PostToolUse => "PostToolUse",
    PostToolUseFailure => "PostToolUseFailure",
});
wire_enum!(CallbackId { Guard => "hive-guard", Finished => "hive-finished" });
pub(super) use crate::external_mcp::McpMethod;
wire_enum!(McpServer { Hive => "hive" });
wire_enum!(AssistantError {
    AuthenticationFailed => "authentication_failed", BillingError => "billing_error",
    RateLimit => "rate_limit",
});
wire_enum!(PermissionMode { Default => "default" });
wire_enum!(ApiProvider { FirstParty => "firstParty" });
wire_enum!(ApiKeySource { None => "none", SelectedEnvironment => "ANTHROPIC_API_KEY" });
wire_enum!(ModelKind { Synthetic => "<synthetic>" });
wire_enum!(UserBlockKind { ToolResult => "tool_result" });

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NativeTool {
    Read,
    Glob,
    Grep,
    Bash,
    Edit,
    Write,
    NotebookEdit,
    Skill,
    Hive,
    Other,
}
impl NativeTool {
    pub(crate) fn parse(name: &str) -> Self {
        // Coordination tools are an open namespace; their exact names stay in raw payloads.
        if name.starts_with("mcp__hive__hive_") {
            return Self::Hive;
        }
        match name {
            "Read" => Self::Read,
            "Glob" => Self::Glob,
            "Grep" => Self::Grep,
            "Bash" => Self::Bash,
            "Edit" => Self::Edit,
            "Write" => Self::Write,
            "NotebookEdit" => Self::NotebookEdit,
            "Skill" => Self::Skill,
            _ => Self::Other,
        }
    }
}

#[derive(Deserialize)]
pub(super) struct Settings {
    pub applied: AppliedSettings,
}
#[derive(Deserialize)]
pub(super) struct AppliedSettings {
    pub model: Option<String>,
    pub effort: Option<String>,
}
#[derive(Deserialize)]
pub(super) struct InterruptReceipt {
    pub still_queued: Option<Vec<String>>,
    pub cancelled: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum Frame {
    ControlResponse {
        response: ControlResult,
    },
    ControlRequest {
        request_id: String,
        request: ControlRequest,
    },
    ControlCancelRequest {
        request_id: String,
    },
    System {
        subtype: SystemSubtype,
        #[serde(flatten)]
        fields: System,
    },
    User {
        uuid: Option<String>,
        session_id: String,
        parent_tool_use_id: Option<String>,
        #[serde(default, rename = "isReplay")]
        replay: bool,
        message: Option<UserMessage>,
    },
    Assistant {
        error: Option<AssistantError>,
        message: Assistant,
        session_id: String,
        parent_tool_use_id: Option<String>,
    },
    StreamEvent {
        session_id: String,
        parent_tool_use_id: Option<String>,
        user_message_uuid: Option<String>,
        event: StreamEvent,
    },
    Result {
        session_id: String,
        subtype: ResultSubtype,
        is_error: bool,
        user_message_uuid: Option<String>,
        terminal_reason: Option<TerminalReason>,
        #[serde(rename = "num_turns")]
        _num_turns: Option<u64>,
        origin: Option<Origin>,
    },
    RateLimitEvent {
        session_id: String,
        rate_limit_info: serde_json::Value,
    },
    #[serde(other)]
    Other,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum StreamEvent {
    MessageStart {
        message: StreamMessage,
    },
    // Partial blocks only feed live viewers. Unexpected shapes are ignored rather
    // than failing the session, because they never carry protocol obligations.
    ContentBlockDelta {
        #[serde(default)]
        delta: Option<StreamDelta>,
    },
    #[serde(other)]
    Other,
}
#[derive(Deserialize)]
pub(super) struct StreamMessage {
    pub model: String,
    #[serde(default)]
    pub id: Option<String>,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum StreamDelta {
    TextDelta {
        #[serde(default)]
        text: String,
    },
    ThinkingDelta {
        #[serde(default)]
        thinking: String,
    },
    #[serde(other)]
    Other,
}
#[derive(Deserialize)]
pub(super) struct UserMessage {
    pub content: UserContent,
}
#[derive(Deserialize)]
pub(super) struct ControlResult {
    pub subtype: ControlSubtype,
    pub request_id: String,
    #[serde(default)]
    pub response: Value,
}
#[derive(Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub(super) enum ControlRequest {
    McpMessage {
        server_name: McpServer,
        message: McpRequest,
    },
    HookCallback {
        callback_id: CallbackId,
        input: Hook,
    },
    CanUseTool {
        tool_name: String,
        input: Value,
        decision_reason: Option<Value>,
        blocked_path: Option<Value>,
        tool_use_id: Option<String>,
    },
    #[serde(other)]
    Other,
}
#[derive(Deserialize)]
pub(super) struct McpRequest {
    pub id: Option<Value>,
    pub method: McpMethod,
    #[serde(default)]
    pub params: Value,
}
#[derive(Deserialize)]
pub(super) struct ToolCall {
    pub name: String,
    pub arguments: Value,
}
#[derive(Deserialize)]
pub(super) struct Hook {
    #[serde(rename = "hook_event_name")]
    pub event: HookEvent,
    pub session_id: String,
    pub tool_name: Option<String>,
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub tool_input: Value,
}
#[derive(Deserialize)]
pub(super) struct System {
    pub session_id: Option<String>,
    pub model: Option<String>,
    #[serde(rename = "permissionMode")]
    pub permission_mode: Option<PermissionMode>,
    /// Task frames: the task and the tool call that started it.
    pub task_id: Option<String>,
    pub tool_use_id: Option<String>,
    pub is_backgrounded: Option<bool>,
}
#[derive(Deserialize)]
pub(super) struct Assistant {
    pub id: Option<String>,
    pub model: String,
    pub content: Vec<Block>,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum Block {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(other)]
    Other,
}
#[derive(Deserialize)]
pub(super) struct Origin {
    pub kind: OriginKind,
}
