//! Current export envelopes retain opaque rows and evidence from the retired root.
use serde::Serialize;
use serde_json::{Map, Value};
use std::{borrow::Cow, path::Path};

#[derive(Serialize)]
pub(super) struct Manifest<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub source: &'a Path,
    pub consistent_snapshot: bool,
    pub format:
        hive_input::compatibility::Exact<{ crate::compatibility::CONTROLLER_EXPORT.current }>,
    pub encoding: &'static str,
}
#[derive(Serialize)]
pub(super) struct Report<'a> {
    pub operation: &'static str,
    pub source: &'a Path,
    pub output: &'a Path,
    pub rows: u64,
    pub ownership_evidence: EvidenceSummary,
    pub large_output_files: &'static str,
}
#[derive(Serialize)]
pub(super) struct EvidenceSummary {
    pub records: u64,
    pub unavailable: u64,
    pub consistency: &'static str,
}
#[derive(Serialize)]
pub(super) struct Blob {
    pub encoding: &'static str,
    pub data: String,
}
#[derive(Serialize)]
pub(super) struct Row<'a> {
    pub table: &'a str,
    pub row: Map<String, Value>,
}
#[derive(Serialize)]
pub(super) struct Evidence<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub path: Cow<'a, str>,
    pub path_bytes: Option<&'a [u8]>,
    #[serde(flatten)]
    pub outcome: EvidenceOutcome,
}
#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum EvidenceOutcome {
    Record { record: Value },
    Unavailable { unavailable: String },
}

#[cfg(test)]
mod tests;
