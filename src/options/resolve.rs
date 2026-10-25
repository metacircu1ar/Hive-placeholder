//! One precedence resolver for role defaults, inherited snapshots and spawn values.
use super::{OptionMap, OptionSource, Snapshot, catalog, invalid};
use crate::{config::Provider, error::Result};
use serde_json::Value;

pub struct Layer<'a> {
    pub source: &'a str,
    pub options: &'a OptionMap,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub overlay: Option<&'a super::NativeConfig>,
}
impl Layer<'_> {
    fn values(
        &self,
        provider: Provider,
        clears: bool,
    ) -> Result<(OptionMap, std::collections::BTreeSet<String>)> {
        let mut values = self.options.clone();
        for (key, value) in [("model", self.model), ("effort", self.effort)] {
            if let Some(value) = value
                && values
                    .insert(key.into(), Value::String(value.into()))
                    .is_some()
            {
                return Err(invalid(
                    Some(key),
                    "one value per layer; use either the shortcut or options",
                ));
            }
        }
        let mut overlay = std::collections::BTreeSet::new();
        if let Some(config) = self.overlay {
            for (key, value) in &config.values {
                if values.insert(key.clone(), value.clone()).is_some() {
                    return Err(invalid(
                        Some(key),
                        "one value per layer; do not repeat an option in native configuration",
                    ));
                }
                overlay.insert(key.clone());
            }
        }
        catalog(provider)?.validate(&values, clears)?;
        for key in ["model", "effort"] {
            if values.get(key).is_some_and(Value::is_null) {
                return Err(invalid(
                    Some(key),
                    "a required selection value; only optional spawn settings can be cleared",
                ));
            }
        }
        Ok((values, overlay))
    }
}

fn merge_layer(
    snapshot: &mut Snapshot,
    layer: &Layer<'_>,
    provider: Provider,
    clears: bool,
) -> Result<()> {
    let (values, overlay) = layer.values(provider, clears)?;
    for (key, value) in values {
        if value.is_null() {
            snapshot.values.remove(&key);
            snapshot.sources.remove(&key);
        } else {
            snapshot.values.insert(key.clone(), value);
            snapshot.sources.insert(
                key.clone(),
                OptionSource {
                    source: layer.source.into(),
                    native_overlay: overlay.contains(&key),
                },
            );
        }
    }
    Ok(())
}

/// Resolve lowest to highest priority. Inherited per-key privacy survives a copy.
pub fn resolve(
    provider: Provider,
    defaults: Option<Layer<'_>>,
    inherited: Option<(&Snapshot, &str)>,
    role: Option<Layer<'_>>,
    spawn: Layer<'_>,
) -> Result<Snapshot> {
    let mut snapshot = Snapshot::default();
    for entry in &catalog(provider)?.entries {
        if entry.available && !entry.default.is_null() {
            snapshot
                .values
                .insert(entry.key.clone(), entry.default.clone());
            snapshot
                .sources
                .insert(entry.key.clone(), OptionSource::plain("catalog_default"));
        }
    }
    if let Some(layer) = defaults {
        merge_layer(&mut snapshot, &layer, provider, false).map_err(|cause| {
            crate::error::Error::config_invalid("Invalid configured agent options.")
                .with_source(cause)
        })?;
    }
    if let Some((parent, id)) = inherited {
        catalog(provider)?
            .validate(&parent.values, false)
            .map_err(|cause| {
                crate::error::Error::config_invalid("Invalid inherited agent options.")
                    .with_source(cause)
            })?;
        for (key, value) in &parent.values {
            snapshot.values.insert(key.clone(), value.clone());
            snapshot.sources.insert(
                key.clone(),
                OptionSource {
                    source: format!("inherited:{id}"),
                    native_overlay: parent
                        .sources
                        .get(key)
                        .is_some_and(|source| source.native_overlay),
                },
            );
        }
    }
    if let Some(layer) = role {
        merge_layer(&mut snapshot, &layer, provider, false).map_err(|cause| {
            crate::error::Error::config_invalid("Invalid configured agent options.")
                .with_source(cause)
        })?;
    }
    merge_layer(&mut snapshot, &spawn, provider, true)?;
    Ok(snapshot)
}
