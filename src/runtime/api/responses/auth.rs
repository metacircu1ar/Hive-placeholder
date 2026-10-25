//! Login selection and availability response records.
use crate::{
    config::{AuthMode, Method, Provider},
    runtime::auth_fallback::Availability,
};
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct AuthChangedResponse<'a> {
    pub agent_id: &'a str,
    pub auth: &'a str,
    pub auth_group: Option<&'a str>,
    pub availability: Availability,
    pub takes_effect: &'static str,
}
#[derive(Serialize)]
pub(crate) struct AuthAvailability {
    pub availability: Vec<Availability>,
}
#[derive(Serialize)]
pub(crate) struct AuthGroups<'a> {
    pub groups: Vec<AuthGroup<'a>>,
}
#[derive(Serialize)]
pub(crate) struct AuthGroup<'a> {
    pub name: &'a str,
    pub provider: Provider,
    pub mode: AuthMode,
    pub members: Vec<AuthMember<'a>>,
    pub paid_tail: Vec<AuthMember<'a>>,
}
#[derive(Serialize)]
pub(crate) struct AuthMember<'a> {
    pub profile: &'a str,
    pub method: Method,
    /// Present only in balanced groups; ordered groups ignore weights.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<u16>,
    pub availability: Vec<Availability>,
    pub bound_agents: u64,
    pub active_turns: u64,
}
