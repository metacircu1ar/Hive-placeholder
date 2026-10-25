//! Saved service bindings. Secrets stay in private credential files.
use super::{Error, Result, valid_env_name, valid_name};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CombBinding {
    pub credential_file: PathBuf,
    #[serde(default)]
    pub reviewer: bool,
}
impl CombBinding {
    pub(super) fn validate(&self) -> Result<()> {
        if !self.credential_file.is_absolute() || self.credential_file.as_os_str().len() > 4096 {
            return Err(Error::config_invalid(
                "Comb credential_file must be a bounded absolute path.",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct McpServer {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}
pub type McpServers = BTreeMap<String, McpServer>;
pub(super) fn validate_servers(servers: &McpServers) -> Result<()> {
    let bounded = |s: &str| s.len() <= 4096 && !s.contains('\0');
    if servers.len() > 16
        || servers.iter().any(|(name, server)| {
            !valid_name(name)
                || name == "hive"
                || name.contains("__")
                || server.command.is_empty()
                || !bounded(&server.command)
                || server.args.len() > 64
                || server.env.len() > 64
                || server.args.iter().any(|s| !bounded(s))
                || server
                    .env
                    .iter()
                    .any(|(k, v)| k.len() > 128 || !valid_env_name(k) || !bounded(v))
        })
        || serde_json::to_vec(servers).map_or(true, |v| v.len() > 64 * 1024)
    {
        return Err(Error::config_invalid(
            "Expected at most 16 bounded stdio MCP servers; hive and names containing \
                __ are reserved.",
        ));
    }
    Ok(())
}
