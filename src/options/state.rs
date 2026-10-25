//! Frozen spawn defaults and durable later overrides.
use super::{OptionMap, Snapshot};
use crate::{config::Provider, error::Result};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, sync::Arc};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SelectionStorage {
    #[default]
    BaselineRefs,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOptions {
    #[serde(deserialize_with = "crate::runtime::domain::decode::string_enum")]
    storage: SelectionStorage,
    #[serde(deserialize_with = "crate::runtime::domain::decode::object")]
    pub baseline: Arc<Snapshot>,
    #[serde(default, skip_serializing_if = "OptionMap::is_empty")]
    pub overrides: OptionMap,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub baseline_configs: Vec<super::NativeConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<super::NativeConfig>,
}

/// Context for lossless baseline references in private agent records.
pub trait SnapshotContext {
    fn baseline_snapshot(&self) -> Option<&Arc<Snapshot>>;
}
impl SnapshotContext for AgentOptions {
    fn baseline_snapshot(&self) -> Option<&Arc<Snapshot>> {
        Some(&self.baseline)
    }
}
impl SnapshotContext for OptionsView {
    fn baseline_snapshot(&self) -> Option<&Arc<Snapshot>> {
        None
    }
}
impl AgentOptions {
    pub fn new(baseline: impl Into<Arc<Snapshot>>) -> Self {
        Self {
            storage: SelectionStorage::BaselineRefs,
            baseline: baseline.into(),
            overrides: OptionMap::new(),
            baseline_configs: Vec::new(),
            config: None,
        }
    }
    pub fn with_configs(
        baseline: impl Into<Arc<Snapshot>>,
        baseline_configs: Vec<super::NativeConfig>,
    ) -> Self {
        Self {
            storage: SelectionStorage::BaselineRefs,
            baseline: baseline.into(),
            overrides: OptionMap::new(),
            baseline_configs,
            config: None,
        }
    }
    pub fn desired(&self) -> Snapshot {
        let mut snapshot = self.baseline.as_ref().clone();
        if let Some(config) = &self.config {
            snapshot.merge(
                &config.values,
                &super::OptionSource {
                    source: config.source.clone(),
                    native_overlay: true,
                },
            );
        }
        snapshot.merge(&self.overrides, &super::OptionSource::plain("set_options"));
        snapshot
    }
    pub(crate) fn desired_ref(&self) -> Cow<'_, Snapshot> {
        if self.overrides.is_empty() && self.config.is_none() {
            Cow::Borrowed(&self.baseline)
        } else {
            Cow::Owned(self.desired())
        }
    }
    pub(crate) fn desired_shared(&self) -> Arc<Snapshot> {
        if self.overrides.is_empty() && self.config.is_none() {
            self.baseline.clone()
        } else {
            Arc::new(self.desired())
        }
    }
    pub fn changed(
        &self,
        provider: Provider,
        changes: &OptionMap,
        config: Option<Option<super::NativeConfig>>,
    ) -> Result<Self> {
        let mut result = self.clone();
        if let Some(config) = config {
            result.config = config;
            if let Some(config) = &result.config {
                for key in config.values.keys() {
                    result.overrides.remove(key);
                }
            }
        }
        for (key, value) in changes {
            if value.is_null() {
                result.overrides.remove(key);
                if let Some(config) = &mut result.config {
                    config.values.remove(key);
                }
            } else {
                result.overrides.insert(key.clone(), value.clone());
                if let Some(config) = &mut result.config {
                    config.values.remove(key);
                }
            }
        }
        if result
            .config
            .as_ref()
            .is_some_and(|config| config.values.is_empty())
        {
            result.config = None;
        }
        super::catalog(provider)?.validate(&result.desired().values, false)?;
        Ok(result)
    }

    pub fn inherited_configs(&self, id: &str) -> Vec<super::NativeConfig> {
        self.baseline_configs
            .iter()
            .chain(self.config.iter())
            .map(|config| config.inherited(id))
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionsView {
    pub options: OptionMap,
    pub option_sources: std::collections::BTreeMap<String, super::OptionSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_configs: Vec<super::NativeConfigView>,
}
impl AgentOptions {
    pub fn public(&self) -> OptionsView {
        let desired = self.desired();
        OptionsView {
            options: desired.public_values(),
            option_sources: desired.sources,
            native_configs: self
                .baseline_configs
                .iter()
                .chain(self.config.iter())
                .map(super::NativeConfig::view)
                .collect(),
        }
    }
}

impl AgentOptions {
    pub fn validate_stored(&self, provider: Provider) -> Result<()> {
        self.baseline.validate_stored(provider)?;
        super::catalog(provider)?.validate(&self.overrides, false)?;
        for config in self.baseline_configs.iter().chain(self.config.iter()) {
            config.validate_stored(provider)?;
        }
        if !self.overrides.is_empty() || self.config.is_some() {
            self.desired().validate_stored(provider)?;
        }
        Ok(())
    }
}
