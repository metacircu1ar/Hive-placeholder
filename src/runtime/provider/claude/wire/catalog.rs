//! Project native model descriptors into the shared catalog without key lookups.
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Catalog<'a> {
    pub data: Vec<Entry<'a>>,
    pub next_cursor: Option<&'a str>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry<'a> {
    pub id: &'a str,
    pub model: &'a str,
    pub resolved_model: &'a str,
    pub display_name: &'a str,
    pub description: &'a str,
    pub supported_reasoning_efforts: Vec<Effort<'a>>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Effort<'a> {
    pub reasoning_effort: &'a str,
}
