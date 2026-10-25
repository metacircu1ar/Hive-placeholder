//! Setup results use explicit shapes and retain canonical JSON object order.
use super::{comb, plan};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
pub(super) struct Preview<'a> {
    pub dry_run: bool,
    pub config: &'a Path,
    pub state: &'a Path,
    pub providers: &'a [plan::ProviderPlan],
    pub proposed_config_before_model_discovery: String,
    pub deferred: &'static str,
    pub comb: Option<&'a comb::Details>,
    pub skills: Vec<PathBuf>,
    pub services: bool,
    pub restart_daemon: bool,
}
#[derive(Serialize)]
pub(super) struct Applied {
    #[serde(flatten)]
    pub readiness: Readiness,
    #[serde(flatten)]
    pub publication: Publication,
    pub providers: Vec<plan::ProviderPlan>,
    pub comb: Option<comb::Applied>,
    pub pending: Vec<String>,
    pub doctor: serde_json::Value,
    pub first_spawn: String,
    pub guide: Option<PathBuf>,
}

#[derive(Serialize)]
pub(super) struct Readiness {
    pub configured: bool,
    pub ready: bool,
}

#[derive(Serialize)]
pub(super) struct Publication {
    pub changed: bool,
    pub backup: Option<PathBuf>,
    pub restart_pending: bool,
    pub daemon_restarted: bool,
}
