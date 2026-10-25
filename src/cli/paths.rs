//! Default CLI configuration and state path resolution.
use super::{Error, PathBuf, Result};

impl super::Cli {
    pub(super) fn config_path(&self) -> Result<PathBuf> {
        self.config.clone().map_or_else(
            || default_path("XDG_CONFIG_HOME", ".config").map(|path| path.join("config.yaml")),
            Ok,
        )
    }

    pub(super) fn state_root(&self) -> Result<PathBuf> {
        self.state_dir
            .clone()
            .map_or_else(|| default_path("XDG_STATE_HOME", ".local/state"), Ok)
    }
}

pub(crate) fn default_path(variable: &str, fallback: &str) -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(variable).filter(|v| !v.is_empty()) {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(Error::invalid_path("XDG paths must be absolute."));
        }
        return Ok(path.join("hive"));
    }
    let home = std::env::var_os("HOME").ok_or(Error::invalid_path(
        "HOME is unset; pass explicit configuration and state paths.",
    ))?;
    let home = PathBuf::from(home);
    if !home.is_absolute() {
        return Err(Error::invalid_path(
            "HOME must be absolute or use explicit paths.",
        ));
    }
    Ok(home.join(fallback).join("hive"))
}
