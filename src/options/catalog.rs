//! Embedded researched catalogs; unknown configuration is never passed through.
use super::{OptionMap, bounded, invalid};
use crate::{
    config::Provider,
    error::{Error, Result},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Turn,
    Session,
    Process,
}
impl Scope {
    pub fn takes_effect(self) -> &'static str {
        match self {
            Self::Turn => "next_turn",
            Self::Session => "next_session",
            Self::Process => "process_move",
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    TurnParameter,
    ThreadConfig,
    ProcessConfig,
    Flag,
    Setting,
    Environment,
    ClaudeThinking,
    Unavailable,
    Owned,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    Model,
    Effort,
    Boolean,
    Integer,
    Enum,
    Thinking,
    Refused,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    pub route: Route,
    pub key: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub key: String,
    pub value_type: ValueType,
    pub values: Vec<String>,
    pub minimum: Option<u64>,
    pub maximum: Option<u64>,
    pub scope: Scope,
    pub mapping: Mapping,
    pub overlay_key: Option<String>,
    pub default: Value,
    pub models: String,
    pub owned: bool,
    pub available: bool,
    pub alternative: Option<String>,
    pub verification: String,
    pub reset: String,
    pub reset_value: Value,
    pub description: String,
    pub evidence: Vec<String>,
    pub verified_versions: Vec<String>,
    pub minimum_version: String,
}
#[derive(Debug, Serialize)]
pub struct Catalog {
    pub provider: Provider,
    pub(super) entries: Vec<Entry>,
    pub model_rules: BTreeMap<String, Vec<String>>,
    #[serde(skip)]
    by_key: BTreeMap<String, usize>,
}
impl<'de> Deserialize<'de> for Catalog {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            provider: Provider,
            entries: Vec<Entry>,
            model_rules: BTreeMap<String, Vec<String>>,
        }
        let fields: Fields = crate::runtime::domain::decode::object(deserializer)?;
        let catalog = Self {
            by_key: fields
                .entries
                .iter()
                .enumerate()
                .map(|(index, entry)| (entry.key.clone(), index))
                .collect(),
            provider: fields.provider,
            entries: fields.entries,
            model_rules: fields.model_rules,
        };
        catalog.check().map_err(serde::de::Error::custom)?;
        Ok(catalog)
    }
}
static CATALOGS: OnceLock<Result<[Catalog; 2]>> = OnceLock::new();

pub fn catalog(provider: Provider) -> Result<&'static Catalog> {
    let all = CATALOGS
        .get_or_init(|| {
            let parse = |text| -> Result<Catalog> {
                let value: Catalog = serde_json::from_str(text).map_err(|cause| {
                    Error::internal_error("Invalid embedded option catalog.").with_source(cause)
                })?;
                Ok(value)
            };
            Ok([
                parse(include_str!("catalogs/codex.json"))?,
                parse(include_str!("catalogs/claude.json"))?,
            ])
        })
        .as_ref()
        .map_err(Clone::clone)?;
    Ok(&all[match provider {
        Provider::Codex => 0,
        Provider::Claude => 1,
    }])
}
impl Catalog {
    fn check(&self) -> Result<()> {
        let mut keys = BTreeSet::new();
        let mut native = BTreeSet::new();
        for entry in &self.entries {
            if !keys.insert(&entry.key)
                || entry.evidence.is_empty()
                || entry.verified_versions.is_empty()
                || semver::Version::parse(&entry.minimum_version).is_err()
                || entry
                    .overlay_key
                    .as_ref()
                    .is_some_and(|key| !native.insert(key))
                || (entry.available && entry.scope == Scope::Turn && entry.reset_value.is_null())
            {
                return Err(Error::internal_error(
                    "Invalid embedded option catalog contract.",
                ));
            }
        }
        Ok(())
    }
    pub fn entry(&self, key: &str) -> Result<&Entry> {
        // Never echo unresearched names: they may contain credential or prompt text.
        self.by_key
            .get(key)
            .map(|index| &self.entries[*index])
            .ok_or_else(|| {
                invalid(
                    None,
                    "a key listed by hivectl options catalog for this provider",
                )
            })
    }
    pub fn validate(&self, values: &OptionMap, resets: bool) -> Result<()> {
        bounded(values)?;
        for (key, value) in values {
            let entry = self.entry(key)?;
            if entry.owned || !entry.available {
                return Err(invalid(
                    Some(&entry.key),
                    entry
                        .alternative
                        .as_deref()
                        .unwrap_or("a supported catalog option"),
                ));
            }
            if value.is_null() && resets {
                continue;
            }
            let accepted = match entry.value_type {
                ValueType::Model => value
                    .as_str()
                    .is_some_and(|s| hive_input::validate::model(s).is_ok()),
                ValueType::Effort => value
                    .as_str()
                    .is_some_and(|s| hive_input::validate::effort(s).is_ok()),
                ValueType::Boolean => value.is_boolean(),
                ValueType::Integer => entry.integer(value),
                ValueType::Enum => value
                    .as_str()
                    .is_some_and(|s| entry.values.iter().any(|v| v == s)),
                ValueType::Thinking => {
                    entry.integer(value)
                        || value
                            .as_str()
                            .is_some_and(|s| entry.values.iter().any(|v| v == s))
                }
                ValueType::Refused => false,
            };
            if !accepted {
                return Err(invalid(Some(&entry.key), &entry.expected()));
            }
        }
        Ok(())
    }

    pub(crate) fn validate_version(&self, values: &OptionMap, version: &str) -> Result<()> {
        let observed = semver::Version::parse(version).map_err(|cause| {
            Error::version_unrecognized("Provider version must be a semantic version.")
                .with_source(cause)
        })?;
        for key in values.keys() {
            let entry = self.entry(key)?;
            let minimum = semver::Version::parse(&entry.minimum_version).map_err(|cause| {
                Error::internal_error("Invalid option catalog minimum version.").with_source(cause)
            })?;
            if observed < minimum {
                return Err(Error::UnsupportedOperation {
                    message: format!(
                        "Option {} requires {} {} or newer; detected {}.",
                        entry.key,
                        self.provider.as_str(),
                        entry.minimum_version,
                        version
                    )
                    .into(),
                    source: None,
                });
            }
        }
        Ok(())
    }
    /// Model names here are the native resolved identity, not an unpinned alias.
    /// Live model/effort membership is checked by the caller's discovered catalog.
    pub fn validate_capabilities(&self, values: &OptionMap, resolved_model: &str) -> Result<()> {
        self.validate(values, false)?;
        for (key, value) in values {
            let entry = self.entry(key)?;
            let supported = match entry.models.as_str() {
                "any" | "live_catalog" | "live_efforts" => true,
                "claude_thinking" => {
                    if value.is_u64() {
                        self.model_in("thinking_budget", resolved_model)
                    } else if value.as_str() == Some("off") {
                        self.model_in("thinking_switch", resolved_model)
                    } else {
                        self.model_in("thinking_switch", resolved_model)
                            || self.model_in("thinking_always_on", resolved_model)
                    }
                }
                rule => self.model_in(rule, resolved_model),
            };
            if !supported {
                return Err(invalid(
                    Some(key),
                    "a setting supported by the selected resolved model; see hivectl options \
                        catalog",
                ));
            }
        }
        if let (Some(budget), Some(limit)) = (
            values.get("thinking").and_then(Value::as_u64),
            values
                .get("claude.max_output_tokens")
                .and_then(Value::as_u64),
        ) && budget >= limit
        {
            return Err(invalid(
                Some("thinking"),
                "a thinking budget below claude.max_output_tokens",
            ));
        }
        Ok(())
    }
    fn model_in(&self, rule: &str, model: &str) -> bool {
        self.model_rules
            .get(rule)
            .is_some_and(|models| models.iter().any(|name| name == model))
    }
}
impl Entry {
    fn integer(&self, value: &Value) -> bool {
        value.as_u64().is_some_and(|n| {
            self.minimum.is_none_or(|min| n >= min) && self.maximum.is_none_or(|max| n <= max)
        })
    }
    fn expected(&self) -> String {
        match self.value_type {
            ValueType::Enum | ValueType::Thinking => {
                let mut expected = self.values.join(", ");
                if let (Some(min), Some(max)) = (self.minimum, self.maximum) {
                    expected = format!("{expected}, or an integer in {min}..{max}");
                }
                expected
            }
            ValueType::Integer => format!(
                "an integer in {}..{}",
                self.minimum.unwrap_or(0),
                self.maximum.unwrap_or(u64::MAX)
            ),
            ValueType::Boolean => "a boolean (true or false)".into(),
            ValueType::Model => "a model token in the selected login's catalog".into(),
            ValueType::Effort => "an effort token supported by the selected model".into(),
            ValueType::Refused => "a setting controlled by Hive".into(),
        }
    }
}

/// Display researched capabilities without credentials, inference or a running daemon.
pub fn listing(provider: Option<Provider>, model: Option<&str>) -> Result<Value> {
    if let Some(model) = model {
        hive_input::validate::model(model).map_err(|_| invalid(Some("model"), "a model token"))?;
    }
    let providers: &[Provider] = match provider {
        Some(Provider::Codex) => &[Provider::Codex],
        Some(Provider::Claude) => &[Provider::Claude],
        None => &[Provider::Codex, Provider::Claude],
    };
    let mut catalogs = Vec::new();
    for &provider in providers {
        let catalog = catalog(provider)?;
        let entries: Vec<_> = catalog
            .entries
            .iter()
            .filter(|entry| {
                model.is_none_or(|model| {
                    !entry.available
                        || matches!(
                            entry.models.as_str(),
                            "any" | "live_catalog" | "live_efforts"
                        )
                        || (entry.models == "claude_thinking"
                            && ["thinking_switch", "thinking_budget", "thinking_always_on"]
                                .iter()
                                .any(|rule| catalog.model_in(rule, model)))
                        || catalog.model_in(&entry.models, model)
                })
            })
            .collect();
        catalogs.push(CatalogDocument {
            provider,
            entries,
            model_rules: &catalog.model_rules,
        });
    }
    Ok(crate::runtime::api::value(CatalogListing {
        catalogs,
        model,
    }))
}

#[derive(Serialize)]
struct CatalogDocument<E, R> {
    provider: Provider,
    entries: E,
    model_rules: R,
}

#[derive(Serialize)]
struct CatalogListing<'a, C> {
    catalogs: C,
    model: Option<&'a str>,
}
