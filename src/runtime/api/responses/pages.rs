//! Agent, event and output pagination responses.
use crate::runtime::domain::EventKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize)]
pub(crate) struct AgentsPage {
    // Agent records are validated before their listing projection is serialized.
    pub agents: Vec<Value>,
    pub next_after: Option<i64>,
}
#[derive(Serialize)]
pub(crate) struct EventRecord<'a> {
    pub seq: i64,
    pub agent_id: &'a str,
    pub execution_id: Option<String>,
    pub timestamp: i64,
    #[serde(rename = "type")]
    pub kind: EventKind,
    pub data: Value,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EventsPage<Items = Vec<Value>> {
    pub events: Items,
    pub cursor: u64,
    pub pruned_through: u64,
    pub history_expired: bool,
}
/// Retained history before a cursor, oldest first; `older` continues backwards.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HistoryPage<Items = Vec<Value>> {
    pub events: Items,
    pub older: Option<u64>,
    pub pruned_through: u64,
    pub history_expired: bool,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OutputResponse {
    pub artifact_id: String,
    pub offset: u64,
    pub text: String,
    pub next_offset: u64,
    pub bytes: u64,
    pub eof: bool,
}
