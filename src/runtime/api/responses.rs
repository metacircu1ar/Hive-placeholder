//! Typed response exports and command acknowledgements.
use crate::error::Error;
use serde::Serialize;
use serde_json::Value;

mod auth;
pub(crate) use auth::{AuthAvailability, AuthChangedResponse, AuthGroup, AuthGroups, AuthMember};
mod calls;
mod gc;
mod read;
mod roles;
pub(crate) use calls::{CallOutcome, MessageResult, OutputSelection};
pub(crate) use gc::{GC_PREVIEW, GcPage, GcStats};
pub(crate) use read::DaemonReply;
pub(crate) use roles::RolesResponse;

#[expect(
    clippy::expect_used,
    reason = "Runtime response payloads contain JSON-compatible fields and string-keyed objects."
)]
pub(crate) fn value(response: impl Serialize) -> Value {
    serde_json::to_value(response).expect("runtime responses must be JSON serializable")
}

#[derive(Serialize)]
pub(crate) struct ErrorResponse {
    pub error: Error,
}
#[derive(serde::Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BatchResponse {
    pub results: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(
        default,
        deserialize_with = "crate::runtime::domain::decode::optional_object"
    )]
    pub join: Option<super::joins::JoinAccepted>,
}
#[derive(Serialize)]
pub(crate) struct CompleteResponse<'a> {
    pub agent_id: &'a str,
    pub completion: &'static str,
}
#[derive(Serialize)]
pub(crate) struct TerminateResponse {
    pub agent_id: String,
    pub termination: &'static str,
}
#[derive(Serialize)]
pub(crate) struct ControlResponse {
    pub agent_id: String,
    pub operation: &'static str,
    pub status: &'static str,
}
#[derive(Serialize)]
pub(crate) struct StatusResponse {
    pub status: &'static str,
}
#[derive(Serialize)]
pub(crate) struct OptionsChangedResponse {
    pub agent_id: String,
    pub desired: super::super::domain::OptionsSelection,
    pub takes_effect: std::collections::BTreeMap<String, &'static str>,
}
#[derive(Serialize)]
pub(crate) struct NameChangedResponse<'a> {
    pub agent_id: &'a str,
    pub name: Option<&'a str>,
}
#[derive(Serialize)]
pub(crate) struct SpawnResponse {
    pub agent_id: String,
    pub message_id: String,
    pub status: &'static str,
}

#[derive(Serialize)]
pub(crate) struct ModelsResponse {
    pub models: Value,
}
#[derive(Serialize)]
pub(crate) struct HistoryExpiredResponse<'a> {
    pub error: Error,
    pub pruned_through: &'a Value,
}
#[derive(Serialize)]
pub(crate) struct WaitEventResponse<'a> {
    pub agent: &'a Value,
    pub event: &'a Value,
}
#[derive(Serialize)]
pub(crate) struct WaitObservationResponse<'a> {
    pub wait: &'static str,
    pub agent: &'a Value,
}

mod agents;
pub(crate) use agents::{
    AgentResponse, AttachedMessage, ListingFields, SteeringVerification, VerificationSelection,
    listed_agent,
};

mod status;
pub(crate) use status::{
    Capabilities, CombBacklog, ControlReply, PingResponse, ProviderCapabilities, RuntimeResources,
    SteeringPolicy, StorageDiagnostics, WriterObservation,
};

mod messaging;
pub(crate) use messaging::{
    MessageRecord, MessagesPage, RedirectTarget, SendResponse, SteeringTarget,
};

mod pages;
pub(crate) use pages::{AgentsPage, EventRecord, EventsPage, HistoryPage, OutputResponse};

mod maintenance;
pub(crate) use maintenance::{PurgeBlocker, PurgeCounts, PurgeResponse};

mod live;
pub(crate) use live::{AttachReply, AttachResponse, TreeReply, TreeResponse, TreeRow};

pub(crate) use read::objects;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RequiredBuild {
    Comb,
}
/// A public error envelope whose provider-specific payload is already typed.
#[cfg(feature = "comb")]
#[derive(serde::Serialize)]
pub(crate) struct ValueError<T> {
    pub error: T,
}

#[derive(serde::Serialize)]
pub(crate) struct UsageReply<T> {
    pub usage: T,
}

#[cfg(feature = "comb")]
#[derive(serde::Serialize)]
pub(crate) struct CollectorReply<T> {
    pub collectors: T,
}
