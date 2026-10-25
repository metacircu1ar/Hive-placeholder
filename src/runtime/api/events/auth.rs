//! Login boundary, availability and interrupted-input evidence.
use crate::runtime::auth_fallback::{AuthReason, AvailabilityState, ProbeError};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

#[derive(Serialize)]
pub(crate) struct ApiKeyRotated<'a> {
    pub profile: &'a str,
    pub generation: Uuid,
    pub previous: Option<&'a crate::state::KeyAuthority>,
    pub selected: &'a crate::state::KeyAuthority,
}

#[derive(Serialize)]
pub(crate) struct AuthSwitchRequested<'a> {
    pub auth: &'a str,
    pub takes_effect: &'static str,
}
#[derive(Serialize)]
pub(crate) struct AuthSwitchDeferred<'a> {
    pub error: &'a str,
}
#[derive(Serialize)]
pub(crate) struct AuthSwitched<'a> {
    pub from: &'a str,
    pub to: &'a str,
    pub from_method: &'static str,
    pub to_method: &'static str,
    pub reason: AuthReason,
    pub mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_group: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_decision: Option<crate::runtime::auth_fallback::AuthDecision>,
}
#[derive(Serialize)]
pub(crate) struct BranchContextDropped<'a> {
    pub source: &'a str,
    pub source_turn: &'a str,
    pub reason: AuthReason,
    pub mode: &'static str,
}
#[derive(Serialize)]
pub(crate) struct ContinuationQueued<'a> {
    pub message_id: &'a str,
    pub interrupted_message_id: &'a str,
    pub original_message_id: &'a str,
    pub attempt: u64,
    pub turn_id: &'a str,
}
#[derive(Serialize)]
pub(crate) struct LoginLimited<'a> {
    pub profile: &'a str,
    pub generation: Uuid,
    pub state: AvailabilityState,
    pub until: Option<u64>,
    pub retry_at: Option<u64>,
    pub window: Option<&'a str>,
    pub reason: Option<AuthReason>,
    pub rejected_resets: &'a [Value],
    pub method: &'static str,
}
#[derive(Serialize)]
pub(crate) struct LoginProbeInvalid<'a> {
    pub profile: &'a str,
    pub generation: Uuid,
    pub probe_id: Option<&'a str>,
    pub reason: ProbeError,
}
#[derive(Serialize)]
pub(crate) struct LoginRestored<'a> {
    pub profile: &'a str,
    pub generation: Uuid,
    pub method: &'static str,
}
#[derive(Serialize)]
pub(crate) struct BranchFailed<'a> {
    pub source: &'a str,
    pub reason: &'a str,
    pub uncertain: bool,
}
#[derive(Serialize)]
pub(crate) struct BranchReady<'a> {
    pub source: &'a str,
    pub source_turn: &'a str,
    pub thread_id: &'a str,
}
