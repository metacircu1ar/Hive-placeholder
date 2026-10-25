//! Typed observation failures with the same flat public error representation.
use crate::error::{Error, ErrorCode};
use serde::{Deserialize, Serialize, Serializer, ser::SerializeStruct};

const DEADLINE_MESSAGE: &str = "Observation deadline elapsed; the message was not cancelled.";

#[derive(Clone, Debug)]
pub(crate) enum ObservationError {
    Input(Box<hive_input::InputError>),
    Deadline { message: String },
    Failure { code: ErrorCode, message: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireError {
    code: ErrorCode,
    message: String,
}

impl<'de> Deserialize<'de> for ObservationError {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Shape {
            Input(hive_input::InputError),
            Operational(WireError),
        }
        // MapAccess is passed directly: untagged buffering retains duplicate
        // entries for the strict decoder instead of collapsing them into Value.
        match crate::runtime::domain::decode::object::<_, Shape>(deserializer)? {
            Shape::Input(input) => Ok(Self::Input(Box::new(input))),
            Shape::Operational(error) if matches!(error.code, ErrorCode::Input(_)) => Err(
                serde::de::Error::custom("Input errors require complete metadata."),
            ),
            Shape::Operational(error) => Ok(error.into()),
        }
    }
}

impl From<WireError> for ObservationError {
    fn from(error: WireError) -> Self {
        match error.code {
            ErrorCode::ObservationDeadline => Self::Deadline {
                message: error.message,
            },
            code => Self::Failure {
                code,
                message: error.message,
            },
        }
    }
}

impl ObservationError {
    pub fn input(&self) -> Option<&hive_input::InputError> {
        match self {
            Self::Input(input) => Some(input),
            _ => None,
        }
    }

    pub fn deadline() -> Self {
        Self::Deadline {
            message: DEADLINE_MESSAGE.into(),
        }
    }

    pub fn is_deadline(&self) -> bool {
        matches!(self, Self::Deadline { .. })
    }

    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Deadline { .. } => ErrorCode::ObservationDeadline,
            Self::Input(input) => ErrorCode::input(input.code()),
            Self::Failure { code, .. } => *code,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Deadline { message } | Self::Failure { message, .. } => message,
            Self::Input(input) => input.message(),
        }
    }
}

impl From<Error> for ObservationError {
    fn from(error: Error) -> Self {
        match error {
            Error::Input { input, .. } => Self::Input(input),
            error => Self::Failure {
                code: error.kind(),
                message: error.message().into(),
            },
        }
    }
}

impl Serialize for ObservationError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Self::Input(input) = self {
            return input.wire().serialize(serializer);
        }
        let mut output = serializer.serialize_struct("ObservationError", 2)?;
        output.serialize_field("code", &self.code())?;
        output.serialize_field("message", self.message())?;
        output.end()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Decoder and wire-byte assertions panic on unexpected errors.
mod tests {
    use super::*;

    #[test]
    fn observation_variants_preserve_flat_wire_bytes_and_deadline_messages() {
        for (code, message, deadline) in [
            (ErrorCode::ProviderFailed, "failed", false),
            (ErrorCode::ObservationDeadline, "custom deadline", true),
            (ErrorCode::ObservationDeadline, "", true),
        ] {
            let json = format!(r#"{{"code":"{}","message":"{message}"}}"#, code.as_str());
            let error: ObservationError = serde_json::from_str(&json).unwrap();
            assert_eq!(error.is_deadline(), deadline);
            assert_eq!(error.code(), code);
            assert_eq!(error.message(), message);
            assert_eq!(serde_json::to_string(&error).unwrap(), json);
            let sequence = format!(r#"["{}","{message}"]"#, code.as_str());
            assert!(serde_json::from_str::<ObservationError>(&sequence).is_err());
        }
        for raw in [
            r#"{"code":"unknown","message":"x"}"#,
            r#"{"code":"provider_failed","message":"x","extra":true}"#,
            r#"{"code":"provider_failed"}"#,
        ] {
            assert!(serde_json::from_str::<ObservationError>(raw).is_err());
        }
        assert_eq!(ObservationError::deadline().message(), DEADLINE_MESSAGE);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Protocol fixtures must fail on unexpected acceptance.
mod input_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn input_metadata_is_required_complete_and_object_only() {
        for code in ["out_of_range", "invalid_path"] {
            let good = json!({"code":code,"message":"Invalid page.","field":"/limit",
                "expected":"1 to 200", "examples":[r#"{"op":"agents","limit":100}"#]});
            let error: ObservationError = serde_json::from_value(good.clone()).unwrap();
            assert!(error.input().is_some());
            assert_eq!(serde_json::to_value(error).unwrap(), good);
            for field in ["code", "message", "field", "expected", "examples"] {
                let mut bad = good.clone();
                bad.as_object_mut().unwrap().remove(field);
                assert!(serde_json::from_value::<ObservationError>(bad).is_err());
            }
            for (field, value) in [
                ("received", json!("secret")),
                ("unknown", json!(null)),
                ("examples", json!([])),
                ("examples", json!(["a", "b", "c", "d"])),
                ("expected", json!("\n")),
                ("message", json!("")),
                ("field", json!(null)),
            ] {
                let mut bad = good.clone();
                bad[field] = value;
                assert!(serde_json::from_value::<ObservationError>(bad).is_err());
            }
            for field in ["code", "message", "field", "expected", "examples"] {
                let text = format!("{{\"{field}\":{},{}", good[field], &good.to_string()[1..]);
                assert!(
                    serde_json::from_str::<ObservationError>(&text).is_err(),
                    "{text}"
                );
            }
            let plain = json!({"code":code,"message":"plain"});
            assert_eq!(
                serde_json::from_value::<ObservationError>(plain).is_ok(),
                code == "invalid_path"
            );
        }
    }
}
