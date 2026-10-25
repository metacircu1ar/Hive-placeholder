//! Immutable resolved settings with provenance; raw overlays have separate storage.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub type OptionMap = BTreeMap<String, Value>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OptionSource {
    pub source: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub native_overlay: bool,
}
impl<'de> Deserialize<'de> for OptionSource {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            source: String,
            #[serde(default)]
            native_overlay: bool,
        }
        let fields: Fields = crate::runtime::domain::decode::object(deserializer)?;
        Ok(Self {
            source: fields.source,
            native_overlay: fields.native_overlay,
        })
    }
}
impl OptionSource {
    pub fn plain(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            native_overlay: false,
        }
    }
}
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub values: OptionMap,
    pub sources: BTreeMap<String, OptionSource>,
}
impl std::fmt::Debug for Snapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Snapshot")
            .field("values", &self.public_values())
            .field("sources", &self.sources)
            .finish()
    }
}
impl Snapshot {
    pub fn string(&self, key: &str) -> Option<&str> {
        self.values.get(key).and_then(Value::as_str)
    }
    pub fn public_values(&self) -> OptionMap {
        self.values
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    if self
                        .sources
                        .get(key)
                        .is_some_and(|source| source.native_overlay)
                    {
                        Value::Null
                    } else {
                        value.clone()
                    },
                )
            })
            .collect()
    }
    #[must_use]
    pub fn public(mut self) -> Self {
        self.values = self.public_values();
        self
    }
    /// Reset to this immutable spawn baseline; never consult current config or parents.
    pub fn changed(
        &self,
        provider: crate::config::Provider,
        overrides: &mut OptionMap,
        changes: &OptionMap,
    ) -> crate::error::Result<Self> {
        let catalog = super::catalog(provider)?;
        catalog.validate(changes, true)?;
        let mut next = overrides.clone();
        for (key, value) in changes {
            if value.is_null() {
                next.remove(key);
            } else {
                next.insert(key.clone(), value.clone());
            }
        }
        let mut desired = self.clone();
        desired.merge(&next, &OptionSource::plain("set_options"));
        catalog.validate(&desired.values, false)?;
        *overrides = next;
        Ok(desired)
    }
    pub(super) fn merge(&mut self, values: &OptionMap, source: &OptionSource) {
        for (key, value) in values {
            self.values.insert(key.clone(), value.clone());
            self.sources.insert(key.clone(), source.clone());
        }
    }
}

impl Snapshot {
    pub fn validate_stored(&self, provider: crate::config::Provider) -> crate::error::Result<()> {
        super::catalog(provider)?.validate(&self.values, false)?;
        if self.values.keys().ne(self.sources.keys())
            || self.sources.values().any(|source| {
                let name = source.source.as_str();
                !matches!(
                    name,
                    "spawn" | "set_options" | "provider_default" | "catalog_default"
                ) && !name
                    .strip_prefix("role:")
                    .is_some_and(crate::config::valid_name)
                    && !name.strip_prefix("inherited:").is_some_and(|id| {
                        !id.is_empty()
                            && id.len() <= 128
                            && id
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                    })
            })
        {
            return Err(crate::error::Error::database_error(
                "Invalid stored option provenance.",
            ));
        }
        Ok(())
    }
}
