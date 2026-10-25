//! Catalog-backed settings and immutable per-agent configuration snapshots.
mod catalog;
mod overlay;
mod process;
mod resolve;
mod snapshot;
mod state;

pub use catalog::{Catalog, Entry, Mapping, Route, Scope, ValueType, catalog, listing};
pub use overlay::{NativeConfig, NativeConfigKind, NativeConfigView};
pub use resolve::{Layer, resolve};
pub use snapshot::{OptionMap, OptionSource, Snapshot};
pub use state::{AgentOptions, OptionsView, SnapshotContext};

use crate::error::{Error, Result};
use hive_input::{Code, InputError};

/// Maximum number of explicitly selected settings on an agent.
pub const MAX_OPTIONS: usize = 64;
/// Maximum size of an accepted native overlay, in UTF-8 bytes.
pub const NATIVE_CONFIG_BYTES: usize = 65536;

fn invalid(key: Option<&str>, expected: &str) -> Error {
    let field = key.map_or_else(|| "/options".to_owned(), |key| format!("/options/{key}"));
    InputError::new(
        Code::InvalidValue,
        "Invalid agent options.",
        &field,
        expected,
        vec![r#"{"op":"set_options","target":"AGENT_ID","options":{"effort":"low"}}"#.into()],
    )
    .map_or_else(
        |cause| Error::internal_error("Invalid option-error metadata.").with_source(cause),
        Error::from,
    )
}

fn bounded(values: &OptionMap) -> Result<()> {
    if values.len() > MAX_OPTIONS {
        return Err(invalid(None, "at most 64 catalog options"));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Controlled catalog and immutable snapshot fixtures.
mod tests;

/// Validate the shared tool vocabulary before an agent's provider is looked up.
/// Provider membership and model capabilities remain checked at the consuming boundary.
pub fn validate_unbound(values: &OptionMap, spawn: bool) -> Result<()> {
    bounded(values)?;
    for (key, value) in values {
        let provider = if key.starts_with("claude.") || key == "thinking" {
            crate::config::Provider::Claude
        } else {
            crate::config::Provider::Codex
        };
        catalog(provider)?.validate(&[(key.clone(), value.clone())].into(), true)?;
        if spawn && matches!(key.as_str(), "model" | "effort") && value.is_null() {
            return Err(invalid(Some(key), "a required spawn selection value"));
        }
    }
    Ok(())
}

pub(crate) fn same_scope(
    provider: crate::config::Provider,
    scope: Scope,
    left: &Snapshot,
    right: &Snapshot,
) -> Result<bool> {
    let catalog = catalog(provider)?;
    Ok(catalog
        .entries
        .iter()
        .filter(|entry| entry.available && !entry.owned && entry.scope == scope)
        .all(|entry| left.values.get(&entry.key) == right.values.get(&entry.key)))
}
