//! Construct typed provider requests and serialize only at the transport boundary.
use super::{ClientMethod, ServerMethod};
use serde_json::Value;
mod params;
mod replies;
mod request;
use params::{
    AccountRead, Capabilities, ClientInfo, Empty, Initialize, InputText, LoadedThreadPage,
    ModelPage, Thread, ThreadFork, ThreadResume, ThreadStart,
};
pub(crate) use params::{
    EffortConfig, EncodedImage, Interrupt, Steer, ThreadBase, TurnInput, TurnStart,
};
pub(crate) use replies::{
    OutgoingFrame, ToolResponse, initialized, request, tool_reply, unsupported,
};
pub(crate) use request::OutgoingRequest;

// Without viewers' live streaming, every delta family stays off.
const ALL_DELTAS: &[ServerMethod] = &[
    ServerMethod::AgentMessageDelta,
    ServerMethod::ReasoningTextDelta,
    ServerMethod::ReasoningSummaryDelta,
    ServerMethod::CommandOutputDelta,
];
// Raw reasoning can exceed visible output several times; it is never streamed.
const RAW_REASONING: &[ServerMethod] = &[ServerMethod::ReasoningTextDelta];

pub(crate) fn initialize(live: bool) -> OutgoingRequest {
    OutgoingRequest::Initialize(Initialize {
        client_info: ClientInfo {
            name: "hive",
            title: "Hive",
            version: env!("CARGO_PKG_VERSION"),
        },
        capabilities: Capabilities {
            experimental_api: true,
            opt_out_notification_methods: if live { RAW_REASONING } else { ALL_DELTAS },
        },
    })
}
pub(crate) fn account_read() -> OutgoingRequest {
    OutgoingRequest::AccountRead(AccountRead {
        refresh_token: false,
    })
}
pub(crate) fn config_requirements() -> OutgoingRequest {
    OutgoingRequest::ConfigRequirements(Empty {})
}
pub(crate) fn config_read() -> OutgoingRequest {
    OutgoingRequest::ConfigRead(Empty {})
}
pub(crate) fn rate_limits_read() -> OutgoingRequest {
    OutgoingRequest::RateLimitsRead(Empty {})
}
#[cfg(test)]
pub(crate) fn models(limit: i32) -> OutgoingRequest {
    OutgoingRequest::ModelList(ModelPage {
        include_hidden: true,
        limit,
        cursor: None,
    })
}
pub(crate) fn model_page(limit: i32, cursor: &Value) -> OutgoingRequest {
    OutgoingRequest::ModelList(ModelPage {
        include_hidden: true,
        limit,
        cursor: Some(cursor.clone()),
    })
}
pub(crate) fn loaded_thread_page(limit: u32, cursor: Option<&str>) -> OutgoingRequest {
    OutgoingRequest::ThreadLoadedList(LoadedThreadPage {
        limit,
        cursor: cursor.map(str::to_owned),
    })
}
pub(crate) fn thread_start(base: Option<ThreadBase>, tools: Option<Value>) -> OutgoingRequest {
    OutgoingRequest::ThreadStart(ThreadStart {
        base,
        dynamic_tools: tools,
    })
}
pub(crate) fn thread_resume(base: Option<ThreadBase>, thread: &str) -> OutgoingRequest {
    OutgoingRequest::ThreadResume(ThreadResume {
        base,
        thread_id: thread.into(),
        exclude_turns: true,
    })
}
pub(crate) fn thread_fork(base: Option<ThreadBase>, source: &str, turn: &str) -> OutgoingRequest {
    OutgoingRequest::ThreadFork(ThreadFork {
        base,
        thread_id: source.into(),
        last_turn_id: turn.into(),
        exclude_turns: true,
        ephemeral: false,
    })
}
pub(crate) fn turn_start(
    thread: &str,
    model: &str,
    effort: &str,
    summary: Option<&str>,
    input: &str,
) -> OutgoingRequest {
    OutgoingRequest::TurnStart(TurnStart {
        thread_id: thread.into(),
        model: Some(model.into()),
        effort: Some(effort.into()),
        summary: summary.map(str::to_owned),
        input: vec![TurnInput::Text { text: input.into() }],
    })
}
pub(crate) fn turn_input(thread: &str, input: &str) -> OutgoingRequest {
    OutgoingRequest::TurnStart(TurnStart {
        thread_id: thread.into(),
        model: None,
        effort: None,
        summary: None,
        input: vec![TurnInput::Text { text: input.into() }],
    })
}
pub(crate) fn steer(thread: &str, turn: &str, input: &str) -> OutgoingRequest {
    OutgoingRequest::TurnSteer(Steer {
        thread_id: thread.into(),
        expected_turn_id: turn.into(),
        input: vec![InputText {
            kind: "text",
            text: input.into(),
        }],
    })
}
pub(crate) fn interrupt(thread: &str, turn: &str) -> OutgoingRequest {
    OutgoingRequest::TurnInterrupt(Interrupt {
        thread_id: thread.into(),
        turn_id: turn.into(),
    })
}
pub(crate) fn terminal_list(thread: &str) -> OutgoingRequest {
    OutgoingRequest::TerminalList(Thread {
        thread_id: thread.into(),
    })
}
pub(crate) fn terminal_clean(thread: &str) -> OutgoingRequest {
    OutgoingRequest::TerminalClean(Thread {
        thread_id: thread.into(),
    })
}
pub(crate) fn thread_unsubscribe(thread: &str) -> OutgoingRequest {
    OutgoingRequest::ThreadUnsubscribe(Thread {
        thread_id: thread.into(),
    })
}
