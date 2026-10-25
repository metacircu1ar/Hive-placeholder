//! Exact native CLI sandbox settings, assembled after all protected paths are validated.
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Settings {
    pub sandbox: Sandbox,
    pub permissions: Permissions,
    pub auto_memory_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub always_thinking_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_compact_window: Option<u64>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Sandbox {
    pub enabled: bool,
    pub fail_if_unavailable: bool,
    #[serde(flatten)]
    pub bash: BashPolicy,
    pub excluded_commands: [String; 0],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<Network>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filesystem: Option<Filesystem>,
}
#[derive(Serialize)]
pub(crate) struct Permissions {
    // Fixed native deny rules for child agents and coordination-bypassing tools.
    pub deny: [&'static str; 7],
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Network {
    pub allow_unix_sockets: [PathBuf; 1],
    pub allow_all_unix_sockets: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Filesystem {
    pub deny_write: Vec<PathBuf>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BashPolicy {
    pub auto_allow_bash_if_sandboxed: bool,
    pub allow_unsandboxed_commands: bool,
}
