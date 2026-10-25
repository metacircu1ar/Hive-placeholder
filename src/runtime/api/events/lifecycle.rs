//! Agent identity, lifetime and control event payloads.
use crate::{
    config::{Lifetime, Provider, Sandbox},
    runtime::{domain::OptionsSelection, hub::Caller},
};
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct Spawned<'a> {
    #[serde(skip_serializing_if = "<[crate::runtime::attachments::Attachment]>::is_empty")]
    pub attachments: &'a [crate::runtime::attachments::Attachment],
    pub parent_id: Option<&'a str>,
    pub role: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'a str>,
    pub provider: Provider,
    pub auth: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_group: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_decision: Option<crate::runtime::auth_fallback::AuthDecision>,
    pub sandbox: Sandbox,
    pub model: &'a str,
    pub effort: &'a str,
    pub lifetime: Lifetime,
    pub initial_message_id: &'a str,
    pub body: &'a str,
}
#[derive(Serialize)]
pub(crate) struct TaskCompleted<'a> {
    pub turn_id: Option<&'a str>,
    pub result: &'a str,
}
#[derive(Serialize)]
pub(crate) struct CompletionRequested<'a> {
    pub by: &'a Caller,
}
#[derive(Serialize)]
pub(crate) struct TerminationRequested<'a> {
    pub by: &'a Caller,
}
#[derive(Serialize)]
pub(crate) struct LifetimeExpired<'a, R> {
    pub parent_id: &'a str,
    pub reason: R,
}
#[derive(Serialize)]
pub(crate) struct OptionsChanged<'a> {
    pub by: &'a Caller,
    pub previous: &'a OptionsSelection,
    pub desired: &'a OptionsSelection,
    pub takes_effect: &'a std::collections::BTreeMap<String, &'static str>,
}
#[derive(Serialize)]
pub(crate) struct ControlRequested<'a> {
    pub operation: &'static str,
    pub by: &'a Caller,
}
#[derive(Serialize)]
pub(crate) struct MemberSkipped<'a> {
    pub auth: &'a str,
    pub reason: &'static str,
}
