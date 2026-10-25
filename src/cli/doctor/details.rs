//! Explicit diagnostic projections; authentication identity is never included.
use crate::{auth, config, error::Error, provider_support};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
pub(super) struct PathDetail<'a> {
    pub path: &'a Path,
}
#[derive(Serialize)]
pub(super) struct SandboxPolicy<'a> {
    pub full_access_roles: &'a [&'a String],
    pub allow_spawn_widening: bool,
}
#[derive(Serialize)]
pub(super) struct ConfigurationChange<'a> {
    pub restart_pending: bool,
    pub loaded: &'a Option<config::ConfigurationIdentity>,
    pub current: Option<&'a config::ConfigurationIdentity>,
}
#[derive(Serialize)]
pub(super) struct DaemonDetail<'a> {
    pub product_version: &'a str,
    pub compiled_features: &'a [String],
    pub status: &'a str,
}
#[derive(Serialize)]
pub(super) struct Login<'a> {
    pub provider: config::Provider,
    pub state: auth::Status,
    pub provider_available: bool,
    pub executable: &'a Option<PathBuf>,
    pub version: &'a Option<String>,
    pub diagnostic: &'a Option<Error>,
    pub warnings: &'a [Error],
}
#[derive(Serialize)]
pub(super) struct ProviderPolicy<'a> {
    pub registry: &'a provider_support::Registry,
    pub observation: &'a Option<provider_support::Observation>,
}
#[derive(Serialize)]
pub(super) struct Installed<'a, T> {
    pub path: &'a Path,
    pub identity: &'a T,
}
#[derive(Serialize)]
pub(super) struct ProbeFailure<'a> {
    pub path: &'a Path,
    pub message: &'static str,
}
#[derive(Serialize)]
pub(super) struct Skills {
    pub hive: PathBuf,
    pub comb_curator: PathBuf,
}
#[derive(Serialize)]
pub(super) struct StaleSkills {
    pub stale: Vec<PathBuf>,
}
