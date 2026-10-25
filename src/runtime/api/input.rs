//! Deferred validation for command-boundary fields.
use super::super::invalid_request;
use crate::error::Result;
use hive_input::Code;
mod context;
pub(crate) use context::InputContext;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned};
use serde_json::Value;
use std::borrow::Cow;
// Maximum string input length, checked only when the field is consumed.
pub(super) const INPUT_STRING_BYTES: usize = crate::limits::CONTROL_INPUT_BYTES;

/// A boundary field whose validation is deferred until the operation consumes it.
/// Mutating requests resolve replay and ID conflicts before lifecycle checks.
/// Handlers interleave limits, lookups and field validation in their defined order,
/// preserving retries across rule changes and the precedence of public errors.
/// Agent tool arguments already pass `validate_tool` before dispatch.
/// Consumers choose whether invalid optional fields are rejected; pagination is strict.
/// Missing and explicit null
/// remain distinct when the CLI serializes a request.
#[derive(Clone, Debug, Default)]
pub enum Input<T> {
    #[default]
    Missing,
    Valid(T),
    Invalid(Value),
}
impl<T> Input<T> {
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
    pub fn get(&self) -> Option<&T> {
        match self {
            Self::Valid(value) => Some(value),
            _ => None,
        }
    }
    pub fn nullable(value: Option<T>) -> Self {
        value.map_or(Self::Invalid(Value::Null), Self::Valid)
    }
}
impl<T: DeserializeOwned> Input<T> {
    pub(crate) fn optional(
        &self,
        context: InputContext,
        field: &'static str,
        expected: &'static str,
    ) -> Result<Option<&T>> {
        match self {
            Self::Missing => Ok(None),
            Self::Valid(value) => Ok(Some(value)),
            Self::Invalid(raw) => {
                let error = context.error(Code::InvalidValue, field, expected);
                Err(match serde_json::from_value::<T>(raw.clone()) {
                    Err(cause) => error.with_source(cause),
                    Ok(_) => error,
                })
            }
        }
    }

    pub fn from_value(raw: Value) -> Self {
        match serde_json::from_value(raw.clone()) {
            Ok(value) => Self::Valid(value),
            Err(_) => Self::Invalid(raw),
        }
    }
}
impl<T> From<T> for Input<T> {
    fn from(value: T) -> Self {
        Self::Valid(value)
    }
}
impl Input<String> {
    pub fn as_str(&self) -> Option<&str> {
        self.get().map(String::as_str)
    }
    /// Native provider identifiers retain their provider-protocol error behavior.
    pub(crate) fn protocol_text(&self) -> Result<&str> {
        self.as_str()
            .filter(|value| !value.is_empty() && value.len() <= INPUT_STRING_BYTES)
            .ok_or_else(invalid_request)
    }
    pub(crate) fn required(&self, context: InputContext, field: &'static str) -> Result<&str> {
        let expected = "a string within 1..65536 UTF-8 bytes";
        let value = self
            .optional(context, field, expected)?
            .ok_or_else(|| context.error(Code::MissingArgument, field, expected))?;
        hive_input::validate::text_bytes(value, INPUT_STRING_BYTES)
            .map_err(|rule| context.error(rule.code(), field, rule.expected()))?;
        Ok(value)
    }
}
impl<T: Copy> Input<T> {
    pub fn copied(&self) -> Option<T> {
        self.get().copied()
    }
}
impl<T: Serialize> Serialize for Input<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Missing => serializer.serialize_none(),
            Self::Valid(value) => value.serialize(serializer),
            Self::Invalid(raw) => raw.serialize(serializer),
        }
    }
}
impl<'de, T: DeserializeOwned> Deserialize<'de> for Input<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Value::deserialize(deserializer).map(Self::from_value)
    }
}
impl<T: JsonSchema> JsonSchema for Input<T> {
    fn inline_schema() -> bool {
        T::inline_schema()
    }
    fn schema_name() -> Cow<'static, str> {
        T::schema_name()
    }
    fn schema_id() -> Cow<'static, str> {
        T::schema_id()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        T::json_schema(generator)
    }
}
