//! Explicit opt-in destinations for shared learning evidence.
use super::{CombBinding, Error, Result, valid_name};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionSource {
    Review,
    Correction,
    TaskCompleted,
    TurnFailed,
    ApprovalAnswer,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectorConfig {
    pub credential_file: PathBuf,
    pub sources: BTreeSet<CollectionSource>,
    #[serde(default = "enabled")]
    pub enabled: bool,
}
fn enabled() -> bool {
    true
}
pub(super) fn validate(collectors: &BTreeMap<String, CollectorConfig>) -> Result<()> {
    if collectors.len() > 16 || collectors.keys().any(|name| !valid_name(name)) {
        return Err(Error::config_invalid(
            "At most 16 named Comb collectors are allowed.",
        ));
    }
    for collector in collectors.values() {
        CombBinding {
            credential_file: collector.credential_file.clone(),
            reviewer: false,
        }
        .validate()?;
    }
    Ok(())
}
