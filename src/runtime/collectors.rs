//! Opt-in evidence collection: durable bounded outbox, asynchronous delivery.
mod delivery;
mod evidence;
pub(super) use delivery::run;
pub(crate) use evidence::{Evidence, EvidenceKind, SUMMARY_BYTES, Verdict};
use serde::{Deserialize, Serialize};

mod admin;
pub(crate) use admin::{Admin, AdminAction};

#[derive(Clone, Debug)]
pub(super) struct Destination {
    pub name: String,
    pub credential_file: std::path::PathBuf,
    pub revision: i64,
}
#[derive(Clone, Debug)]
pub(super) struct Delivery {
    pub id: String,
    pub destination: String,
    pub generation: i64,
    pub revision: i64,
    pub body: String,
}
#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct Status {
    pub name: String,
    pub enabled: bool,
    pub paused: bool,
    pub comb_id: Option<String>,
    pub credential_id: Option<String>,
    pub pending: i64,
    pub bytes: i64,
    pub uncertain: i64,
    pub gap_count: i64,
    pub gap_first: Option<i64>,
    pub gap_last: Option<i64>,
    pub last_error: Option<String>,
    pub uncertain_items: Vec<UncertainItem>,
}
#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct UncertainItem {
    pub id: String,
    pub generation: i64,
    pub comb_id: String,
    pub credential_id: String,
    pub last_error: Option<String>,
}
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Disposition {
    Queued,
    Full,
    Disabled,
    NotSelected,
}
#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct Admission {
    pub event: i64,
    pub destinations: std::collections::BTreeMap<String, Disposition>,
}
