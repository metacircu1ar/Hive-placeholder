//! Strict, bounded native configuration snapshots mapped onto catalog identities.
use super::{NATIVE_CONFIG_BYTES, OptionMap, catalog};
use crate::{
    config::Provider,
    error::{Error, Result},
};
use hive_input::{Code, InputError};
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeConfigKind {
    CodexToml,
    ClaudeJson,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeConfig {
    pub kind: NativeConfigKind,
    pub content: String,
    pub sha256: String,
    pub source: String,
    pub values: OptionMap,
}
impl std::fmt::Debug for NativeConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeConfig")
            .field("kind", &self.kind)
            .field("sha256", &self.sha256)
            .field("source", &self.source)
            .field("keys", &self.values.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeConfigView {
    pub kind: NativeConfigKind,
    pub sha256: String,
    pub source: String,
    pub keys: Vec<String>,
}

impl NativeConfig {
    pub fn parse(
        provider: Provider,
        content: &str,
        source: impl Into<String>,
    ) -> Result<Option<Self>> {
        if content.len() > NATIVE_CONFIG_BYTES {
            return Err(invalid_config("native configuration no larger than 64 KiB"));
        }
        let kind = match provider {
            Provider::Codex => NativeConfigKind::CodexToml,
            Provider::Claude => NativeConfigKind::ClaudeJson,
        };
        let parsed = match provider {
            Provider::Codex => {
                if content.trim().is_empty() {
                    return Ok(None);
                }
                // Parser diagnostics can contain the source line. Keep native
                // configuration private even when parsing fails.
                let value: toml::Value = toml::from_str(content)
                    .map_err(|_| invalid_config("strict Codex TOML using cataloged scalar keys"))?;
                toml_value(value)
            }
            Provider::Claude => {
                let mut decoder = serde_json::Deserializer::from_str(content);
                StrictJson::deserialize(&mut decoder)
                    .and_then(|value| decoder.end().map(|()| value.0))
                    .map_err(|_| invalid_config("a strict Claude settings JSON object"))?
            }
        };
        let object = parsed
            .as_object()
            .ok_or_else(|| invalid_config("a native configuration object"))?;
        if object.is_empty() {
            return Ok(None);
        }
        let mut leaves = Vec::new();
        flatten(None, &parsed, &mut leaves)?;
        let native = catalog(provider)?;
        let mut values = OptionMap::new();
        for (path, mut value) in leaves {
            let entry = native
                .entries
                .iter()
                .find(|entry| entry.overlay_key.as_deref() == Some(path.as_str()))
                .ok_or_else(|| {
                    invalid_config(
                        "only native keys listed by hivectl options catalog for this provider",
                    )
                })?;
            if matches!(entry.key.as_str(), "model" | "effort") {
                return Err(invalid_config(
                    "native configuration without model or effort; use --model/--effort or \
                        typed options",
                ));
            }
            if provider == Provider::Claude && entry.key == "thinking" {
                value = match value.as_bool() {
                    Some(true) => Value::String("on".into()),
                    Some(false) => Value::String("off".into()),
                    None => value,
                };
            }
            if values.insert(entry.key.clone(), value).is_some() {
                return Err(invalid_config("one native spelling per catalog option"));
            }
        }
        native.validate(&values, false)?;
        Ok(Some(Self {
            kind,
            content: content.into(),
            sha256: format!("{:x}", Sha256::digest(content.as_bytes())),
            source: source.into(),
            values,
        }))
    }

    pub fn view(&self) -> NativeConfigView {
        NativeConfigView {
            kind: self.kind,
            sha256: self.sha256.clone(),
            source: self.source.clone(),
            keys: self.values.keys().cloned().collect(),
        }
    }

    #[must_use]
    pub fn inherited(&self, id: &str) -> Self {
        let mut inherited = self.clone();
        inherited.source = format!("inherited:{id}");
        inherited
    }

    pub fn validate_stored(&self, provider: Provider) -> Result<()> {
        let parsed = Self::parse(provider, &self.content, self.source.clone())
            .map_err(|_| Error::database_error("Stored native configuration is invalid."))?
            .ok_or_else(|| Error::database_error("Stored native configuration is empty."))?;
        if parsed.kind != self.kind
            || parsed.sha256 != self.sha256
            || self
                .values
                .iter()
                .any(|(key, value)| parsed.values.get(key) != Some(value))
            || !valid_source(&self.source)
        {
            return Err(Error::database_error(
                "Stored native configuration is inconsistent.",
            ));
        }
        Ok(())
    }
}

fn valid_source(source: &str) -> bool {
    matches!(source, "spawn" | "set_options")
        || source
            .strip_prefix("role:")
            .is_some_and(crate::config::valid_name)
        || source.strip_prefix("inherited:").is_some_and(|id| {
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
}

fn invalid_config(expected: &str) -> Error {
    InputError::new(
        Code::InvalidValue,
        "Invalid native configuration.",
        "/config",
        expected,
        vec![r#"{"op":"set_options","target":"AGENT_ID","config":"..."}"#.into()],
    )
    .map_or_else(
        |cause| Error::internal_error("Invalid native-config error metadata.").with_source(cause),
        Error::from,
    )
}

fn flatten(prefix: Option<&str>, value: &Value, out: &mut Vec<(String, Value)>) -> Result<()> {
    if let Value::Object(values) = value {
        if values.is_empty() && prefix.is_some() {
            return Err(invalid_config(
                "nonempty cataloged native configuration tables",
            ));
        }
        for (key, value) in values {
            let path = prefix.map_or_else(|| key.clone(), |prefix| format!("{prefix}.{key}"));
            flatten(Some(&path), value, out)?;
        }
        return Ok(());
    }
    let path = prefix.ok_or_else(|| invalid_config("a native configuration object"))?;
    out.push((path.into(), value.clone()));
    Ok(())
}

fn toml_value(value: toml::Value) -> Value {
    match value {
        toml::Value::String(value) => Value::String(value),
        toml::Value::Integer(value) => Value::Number(value.into()),
        toml::Value::Float(value) => Number::from_f64(value).map_or(Value::Null, Value::Number),
        toml::Value::Boolean(value) => Value::Bool(value),
        toml::Value::Datetime(value) => Value::String(value.to_string()),
        toml::Value::Array(values) => Value::Array(values.into_iter().map(toml_value).collect()),
        toml::Value::Table(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, toml_value(value)))
                .collect(),
        ),
    }
}

struct StrictJson(Value);
impl<'de> Deserialize<'de> for StrictJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        deserializer.deserialize_any(StrictVisitor)
    }
}
struct StrictVisitor;
impl<'de> de::Visitor<'de> for StrictVisitor {
    type Value = StrictJson;
    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JSON without duplicate object keys")
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> std::result::Result<Self::Value, E> {
        Ok(StrictJson(Value::Bool(value)))
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> std::result::Result<Self::Value, E> {
        Ok(StrictJson(Value::Number(value.into())))
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> std::result::Result<Self::Value, E> {
        Ok(StrictJson(Value::Number(value.into())))
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> std::result::Result<Self::Value, E> {
        Number::from_f64(value)
            .map(|value| StrictJson(Value::Number(value)))
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Self::Value, E> {
        Ok(StrictJson(Value::String(value.into())))
    }
    fn visit_string<E: de::Error>(self, value: String) -> std::result::Result<Self::Value, E> {
        Ok(StrictJson(Value::String(value)))
    }
    fn visit_none<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictJson(Value::Null))
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictJson(Value::Null))
    }
    fn visit_seq<A: de::SeqAccess<'de>>(
        self,
        mut sequence: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<StrictJson>()? {
            values.push(value.0);
        }
        Ok(StrictJson(Value::Array(values)))
    }
    fn visit_map<A: de::MapAccess<'de>>(
        self,
        mut object: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let mut keys = BTreeSet::new();
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(de::Error::custom("duplicate JSON key"));
            }
            values.insert(key, object.next_value::<StrictJson>()?.0);
        }
        Ok(StrictJson(Value::Object(values)))
    }
}
