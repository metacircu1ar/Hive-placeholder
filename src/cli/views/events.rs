//! Borrowed event projections used only for transcript rendering.
use super::{field, object, rows};
use serde_json::Value;

#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Event<'a> {
    #[serde(deserialize_with = "field")]
    pub seq: Option<u64>,
    #[serde(borrow, rename = "type", deserialize_with = "field")]
    pub kind: Option<&'a str>,
    #[serde(borrow, deserialize_with = "object")]
    pub data: Payload<'a>,
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Payload<'a> {
    #[serde(borrow, deserialize_with = "field")]
    pub parent_id: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub body: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub mode: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub model: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub effort: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub turn_id: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub status: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub approval_id: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub title: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub action: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub item_id: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub text: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub aggregation: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub error: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub auth: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub from: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub to: Option<&'a str>,
    #[serde(deserialize_with = "field")]
    pub sender: Value,
    #[serde(borrow, deserialize_with = "object")]
    pub item: ProviderItem<'a>,
    #[serde(borrow, deserialize_with = "object")]
    pub artifact: Artifact<'a>,
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Artifact<'a> {
    #[serde(borrow, deserialize_with = "field")]
    pub id: Option<&'a str>,
    #[serde(deserialize_with = "field")]
    pub bytes: Option<u64>,
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub(in crate::cli) struct ProviderItem<'a> {
    #[serde(borrow, deserialize_with = "field")]
    pub id: Option<&'a str>,
    #[serde(borrow, rename = "type", deserialize_with = "field")]
    pub kind: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub status: Option<&'a str>,
    #[serde(rename = "exitCode", deserialize_with = "field")]
    pub exit_code: Option<i64>,
    #[serde(borrow, rename = "aggregatedOutput", deserialize_with = "field")]
    pub aggregated_output: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub server: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub tool: Option<&'a str>,
    #[serde(deserialize_with = "field")]
    pub command: Value,
    #[serde(borrow, deserialize_with = "rows")]
    pub changes: Vec<Change<'a>>,
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Change<'a> {
    #[serde(borrow, deserialize_with = "field")]
    pub path: Option<&'a str>,
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub(in crate::cli) struct LiveItem<'a> {
    #[serde(borrow, deserialize_with = "field")]
    pub item_id: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub turn_id: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub text: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub kind: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub status: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    pub tool: Option<&'a str>,
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Page<'a> {
    #[serde(borrow, deserialize_with = "rows")]
    pub events: Vec<Event<'a>>,
    #[serde(deserialize_with = "field")]
    pub cursor: Option<u64>,
    #[serde(deserialize_with = "field")]
    pub older: Option<u64>,
    #[serde(deserialize_with = "field")]
    pub history_expired: bool,
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub(in crate::cli) struct Snapshot<'a> {
    #[serde(borrow, deserialize_with = "object")]
    pub events: Page<'a>,
}
