//! Agent metadata shared by attach, the sidebar and pane controllers.
use super::{field, object, present, rows};
use serde::Deserialize;

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Agent<'a> {
    #[serde(borrow, deserialize_with = "field")]
    pub id: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub name: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub role: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub parent_id: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub state: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub task_outcome: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub sandbox: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub model: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub effort: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub auth: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub auth_generation: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub waiting_on: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub waiting_reason: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub approval_id: Option<&'a str>,
    #[serde(borrow, deserialize_with = "object")]
    pub auth_decision: Decision<'a>,
    #[serde(borrow, deserialize_with = "object")]
    pub profile: Profile<'a>,
    #[serde(deserialize_with = "field")]
    pub steering_supported: bool,
    #[serde(rename = "ended_at", deserialize_with = "present")]
    pub ended: bool,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Decision<'a> {
    #[serde(borrow, deserialize_with = "field")]
    pub reason: Option<&'a str>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Profile<'a> {
    #[serde(borrow, deserialize_with = "field")]
    pub provider: Option<&'a str>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Attachment<'a> {
    #[serde(borrow, deserialize_with = "object")]
    pub agent: Agent<'a>,
    #[serde(deserialize_with = "field")]
    pub queued: u64,
    #[serde(deserialize_with = "field")]
    pub mail: u64,
    #[serde(deserialize_with = "field")]
    pub children: u64,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Tree<'a> {
    #[serde(borrow, deserialize_with = "rows")]
    pub agents: Vec<Agent<'a>>,
}
