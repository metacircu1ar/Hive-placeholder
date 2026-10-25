//! The serving embedding selection of a managed knowledge root: text search
//! only, or an HTTP embedding endpoint.
pub(crate) mod files;
pub mod input;
mod selection;
mod serve;

use crate::{Error, Result};
pub use selection::{ServeSettings, recover, select_none, serving_space, settings, use_http};
pub(crate) use selection::{
    pending_settings, reconcile_for_setup, recover_locked, select_http_for_setup,
    select_none_for_setup,
};
pub use serve::{ServePlan, plan_server, prepare_server};
use std::path::{Path, PathBuf};

/// Read-only no-op proof for setup while the owned service holds the writer lock.
pub(crate) fn selection_matches(
    root: &Path,
    http: Option<&crate::embedding::HttpConfig>,
    live_space: Option<&str>,
) -> Result<bool> {
    if pending_settings(root)?.is_some() {
        return Ok(false);
    }
    let Some(saved) = settings(root)? else {
        return Ok(false);
    };
    if let Some(http) = http
        && serde_json::to_vec(&saved.embeddings)?
            != serde_json::to_vec(&Some(crate::embedding::Config::Http(http.clone())))?
    {
        return Ok(false);
    }
    Ok(serving_space(&saved)?
        .map(|s| s.fingerprint().map(|f| f.to_string()))
        .transpose()?
        .as_deref()
        == live_space)
}

pub fn validate_http(config: crate::embedding::HttpConfig) -> Result<()> {
    let mut engine = crate::embedding::Engine::load(crate::embedding::Config::Http(config))?;
    selection::probe_engine(&mut engine)
}

fn invalid(message: impl Into<std::borrow::Cow<'static, str>>) -> Error {
    Error::provision_configuration(message)
}

#[cfg(test)]
mod tests;
