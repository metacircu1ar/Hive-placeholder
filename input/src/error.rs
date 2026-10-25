use crate::Examples;
use serde::{Deserialize, Deserializer, Serialize, de};
use std::fmt;

/// Specific input rejection codes, separate from operational failures.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    UnknownArgument,
    MissingArgument,
    ConflictingArguments,
    InvalidValue,
    OutOfRange,
    InvalidDuration,
    InvalidRequestId,
    InvalidRunName,
    InvalidModel,
    InvalidEffort,
    InvalidUuid,
    InvalidPath,
    InvalidFile,
    InvalidJson,
}

impl Code {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnknownArgument => "unknown_argument",
            Self::MissingArgument => "missing_argument",
            Self::ConflictingArguments => "conflicting_arguments",
            Self::InvalidValue => "invalid_value",
            Self::OutOfRange => "out_of_range",
            Self::InvalidDuration => "invalid_duration",
            Self::InvalidRequestId => "invalid_request_id",
            Self::InvalidRunName => "invalid_run_name",
            Self::InvalidModel => "invalid_model",
            Self::InvalidEffort => "invalid_effort",
            Self::InvalidUuid => "invalid_uuid",
            Self::InvalidPath => "invalid_path",
            Self::InvalidFile => "invalid_file",
            Self::InvalidJson => "invalid_json",
        }
    }
}

/// Invalid error metadata is a programming/protocol error, not an input echo.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvalidMetadata {
    Message,
    Field,
    Expected,
    Examples,
}
impl fmt::Display for InvalidMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Message => "Invalid input-error message.",
            Self::Field => "Invalid input-error field.",
            Self::Expected => "Invalid input-error expected form.",
            Self::Examples => "Invalid input-error examples.",
        })
    }
}
impl std::error::Error for InvalidMetadata {}

/// An error with bounded, trusted metadata and an optional admitted public value.
///
/// Serialization is deliberately explicit; use [`Self::wire`] for daemon/MCP
/// responses, or [`Self::render`](crate::InputError::render) for local CLI output.
///
/// ```compile_fail
/// # fn serialize(error: hive_input::InputError) {
/// serde_json::to_string(&error).unwrap();
/// # }
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputError {
    pub(crate) code: Code,
    pub(crate) message: String,
    pub(crate) field: String,
    pub(crate) expected: String,
    pub(crate) received: Option<String>,
    pub(crate) examples: Examples,
}

impl InputError {
    pub fn new(
        code: Code,
        message: impl Into<String>,
        field: impl Into<String>,
        expected: impl Into<String>,
        examples: Vec<String>,
    ) -> Result<Self, InvalidMetadata> {
        let value = Self {
            code,
            message: message.into(),
            field: field.into(),
            expected: expected.into(),
            received: None,
            examples: Examples::new(examples)?,
        };
        for (text, max, error) in [
            (&value.message, 1024, InvalidMetadata::Message),
            (&value.field, 256, InvalidMetadata::Field),
            (&value.expected, 1024, InvalidMetadata::Expected),
        ] {
            if !metadata(text, max) {
                return Err(error);
            }
        }
        Ok(value)
    }

    /// Call only for values the command/tool registry classifies as public.
    /// Never admit secrets, credentials, message bodies or file contents.
    #[must_use]
    pub fn with_public_received(mut self, value: &str) -> Self {
        self.received = Some(escape_received(value));
        self
    }

    pub const fn code(&self) -> Code {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
    pub fn field(&self) -> &str {
        &self.field
    }
    pub fn expected(&self) -> &str {
        &self.expected
    }
    pub fn examples(&self) -> &[String] {
        self.examples.as_slice()
    }

    /// The only serializable network view. It never includes received input.
    pub fn wire(&self) -> WireError<'_> {
        WireError {
            code: self.code,
            message: &self.message,
            field: &self.field,
            expected: &self.expected,
            examples: self.examples.as_slice(),
        }
    }
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} Expected: {}",
            self.field, self.message, self.expected
        )
    }
}
impl std::error::Error for InputError {}

/// Borrowed, privacy-preserving wire fields. Constructed only from valid metadata.
#[derive(Debug, Serialize)]
pub struct WireError<'a> {
    code: Code,
    message: &'a str,
    field: &'a str,
    expected: &'a str,
    examples: &'a [String],
}

impl<'de> Deserialize<'de> for InputError {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WireOwned {
            code: Code,
            message: String,
            field: String,
            expected: String,
            examples: Vec<String>,
        }
        struct Object;
        impl<'de> de::Visitor<'de> for Object {
            type Value = InputError;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an input-error object")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
                // Preserve duplicate keys for the strict derived decoder.
                let wire = WireOwned::deserialize(de::value::MapAccessDeserializer::new(map))?;
                InputError::new(
                    wire.code,
                    wire.message,
                    wire.field,
                    wire.expected,
                    wire.examples,
                )
                .map_err(de::Error::custom)
            }
        }
        deserializer.deserialize_map(Object)
    }
}

pub(crate) fn metadata(text: &str, max: usize) -> bool {
    !text.trim().is_empty() && text.len() <= max && !text.chars().any(char::is_control)
}

fn escape_received(value: &str) -> String {
    const LIMIT: usize = 200;
    const MARKER: &str = "...";
    let mut result = String::new();
    let mut truncate_at = 0;
    for character in value.chars() {
        let fragment = if character.is_control() || character == '\\' {
            character.escape_default().to_string()
        } else {
            character.to_string()
        };
        if result.len() + fragment.len() > LIMIT {
            result.truncate(truncate_at);
            result.push_str(MARKER);
            return result;
        }
        result.push_str(&fragment);
        if result.len() <= LIMIT - MARKER.len() {
            truncate_at = result.len();
        }
    }
    result
}
