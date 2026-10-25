//! Agent presentation and steering capability responses.
use super::{read, value};
use crate::runtime::domain::Agent;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize)]
pub(crate) struct ListingFields<'a> {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_for_build: Option<crate::runtime::api::RequiredBuild>,

    pub idle_since: Option<u64>,
    pub provider: crate::config::Provider,
    pub billing: crate::config::Method,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_on: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<&'a str>,
}

pub(crate) fn listed_agent(
    agent: &Agent,
    idle_since: Option<u64>,
    approval_id: Option<&str>,
) -> Value {
    let Value::Object(mut fields) = value(agent.clone().public()) else {
        unreachable!("Agent serializes as an object");
    };
    for name in [
        "instructions",
        "profile",
        "completion_pending",
        "result",
        "handoff",
    ] {
        fields.remove(name);
    }
    let Value::Object(listing) = value(ListingFields {
        held_for_build: crate::runtime::features::required_build(agent),
        idle_since,
        provider: agent.profile.provider,
        billing: agent.profile.method,
        waiting_on: approval_id.map(|_| "human"),
        approval_id,
    }) else {
        unreachable!("ListingFields serializes as an object");
    };
    fields.extend(listing);
    Value::Object(fields)
}

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VerificationSelection {
    CurrentSession,
    NextTurn,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SteeringVerification {
    pub auth: String,
    pub model: String,
    pub effort: String,
    #[serde(deserialize_with = "crate::runtime::domain::decode::string_enum")]
    pub selection: VerificationSelection,
    pub daemon_must_be_stopped: bool,
    pub enables_next_launch: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, bound(deserialize = "A: Deserialize<'de>"))]
pub(crate) struct AgentResponse<A = Agent> {
    pub attachment_store_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_attachments: Option<AttachedMessage>,
    #[serde(deserialize_with = "crate::runtime::domain::decode::object")]
    pub agent: A,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "read::objects"
    )]
    pub steering_verification: Vec<SteeringVerification>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AttachedMessage {
    pub message_id: String,
    pub attachments: Vec<crate::runtime::attachments::Attachment>,
}
