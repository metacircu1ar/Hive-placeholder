//! Typed page headers and retained subdocuments handed to the UI worker.
use super::field;
use super::{agents::Agent, object};
use serde::Deserialize;
use serde_json::Value;

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct AgentIdentity<'a> {
    #[serde(borrow, deserialize_with = "field")]
    pub agent_id: Option<&'a str>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct ArtifactOutput<'a> {
    #[serde(borrow, deserialize_with = "field")]
    pub artifact_id: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub text: Option<&'a str>,
    #[serde(deserialize_with = "field")]
    pub offset: Option<u64>,
    #[serde(deserialize_with = "field")]
    pub next_offset: Option<u64>,
    #[serde(deserialize_with = "field")]
    pub bytes: Option<u64>,
    #[serde(deserialize_with = "field")]
    pub eof: Option<bool>,
}

#[derive(Default, Deserialize)]
#[serde(default, bound(deserialize = "Items: Deserialize<'de> + Default"))]
pub(in crate::cli) struct LivePage<Items = Vec<Value>> {
    pub cursor: Value,
    #[serde(deserialize_with = "field")]
    pub items: Items,
    #[serde(deserialize_with = "field")]
    pub resync: bool,
    #[serde(deserialize_with = "field")]
    pub durable_changed: bool,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Listing {
    #[serde(deserialize_with = "field")]
    pub agents: Vec<Value>,
    #[serde(deserialize_with = "field")]
    pub next_after: Option<u64>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct RawAttachment {
    pub events: Value,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Detail {
    pub approval: Value,
    pub usage: Value,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct TreeMetadata {
    pub version: Value,
    #[serde(deserialize_with = "field")]
    pub truncated: bool,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct EventMetadata {
    #[serde(deserialize_with = "field")]
    pub events: Vec<serde::de::IgnoredAny>,
    #[serde(deserialize_with = "field")]
    pub cursor: Option<u64>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct AttachMetadata<'a> {
    #[serde(borrow, deserialize_with = "object")]
    pub agent: Agent<'a>,
    #[serde(deserialize_with = "object")]
    pub events: EventMetadata,
    #[serde(deserialize_with = "field")]
    pub children: u64,
    pub live: Value,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct TerminalPane<'a> {
    #[serde(deserialize_with = "field")]
    pub is_plugin: Option<bool>,
    #[serde(deserialize_with = "field")]
    pub exited: Option<bool>,
    #[serde(deserialize_with = "field")]
    pub tab_id: Option<u64>,
    #[serde(borrow, deserialize_with = "field")]
    pub title: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub terminal_command: Option<&'a str>,
}
