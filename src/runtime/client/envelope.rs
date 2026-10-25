//! Validate daemon envelopes while preserving command and observer response contracts.
use crate::{
    error::{Error, Result},
    runtime::domain::decode,
};
use serde::Deserialize;
use serde_json::Value;

pub(crate) fn invalid() -> Error {
    Error::daemon_protocol_error("Invalid daemon response.")
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ErrorEnvelope {
    #[serde(deserialize_with = "decode::object")]
    pub error: crate::calls::ObservationError,
    // Mutating GC and purge replies include their retry identity, including failures.
    pub request_id: Option<String>,
}

pub(crate) enum Policy {
    Command,
    Observation,
}

pub(crate) fn error(raw: &Value, policy: Policy) -> Result<Option<ErrorEnvelope>> {
    let object = raw.as_object().ok_or_else(invalid)?;
    if !object.contains_key("error") {
        return Ok(None);
    }
    let envelope: ErrorEnvelope =
        decode::object(raw).map_err(|cause| invalid().with_source(cause))?;
    match policy {
        // Commands allow retry identity and require a nonempty message.
        Policy::Command if envelope.error.message().is_empty() => return Err(invalid()),
        // Observations accept only the error, including operational empty messages.
        Policy::Observation if object.len() != 1 => return Err(invalid()),
        _ => {}
    }
    Ok(Some(envelope))
}

pub(super) fn parse(line: &str) -> Result<Value> {
    validate(line, json(line)?)
}

pub(super) fn json(line: &str) -> Result<Value> {
    serde_json::from_str(line).map_err(|cause| invalid().with_source(cause))
}

pub(super) fn validate(line: &str, value: Value) -> Result<Value> {
    if value.get("error").is_some() {
        // Validate the original bytes: a Value projection has already collapsed
        // duplicate keys. Success payloads keep their existing decoding policy.
        serde_json::from_str::<ErrorEnvelope>(line)
            .map_err(|cause| invalid().with_source(cause))?;
    }
    Ok(value)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Wire policy fixtures deliberately panic on unexpected decoding.
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn response_policies_preserve_retry_identity_and_message_requirements() {
        let retry =
            json!({"request_id":"retry","error":{"code":"provider_failed","message":"failed"}});
        assert_eq!(
            error(&retry, Policy::Command)
                .unwrap()
                .unwrap()
                .error
                .message(),
            "failed"
        );
        assert_eq!(
            error(&retry, Policy::Observation).unwrap_err().code(),
            "daemon_protocol_error"
        );
        let empty = json!({"error":{"code":"provider_failed","message":""}});
        assert_eq!(
            error(&empty, Policy::Command).unwrap_err().code(),
            "daemon_protocol_error"
        );
        assert_eq!(
            error(&empty, Policy::Observation)
                .unwrap()
                .unwrap()
                .error
                .message(),
            ""
        );
        for raw in [
            json!(null),
            json!([]),
            json!({"error":null}),
            json!({"error":{"code":"provider_failed","message":"failed","unexpected":true}}),
            json!({"error":{"code":"provider_failed","message":"failed"},"version":1}),
        ] {
            for policy in [Policy::Command, Policy::Observation] {
                assert_eq!(
                    error(&raw, policy).unwrap_err().code(),
                    "daemon_protocol_error"
                );
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Raw JSON is needed to preserve malicious duplicate keys.
mod raw_tests {
    use super::*;
    #[test]
    fn raw_error_objects_reject_duplicates_before_value_projection() {
        let input = r#"{"code":"out_of_range","message":"Invalid page.","field":"/limit","expected":"1 to 200","examples":["hivectl agents --limit 100"]}"#;
        let good = format!(r#"{{"error":{input},"request_id":"retry"}}"#);
        let parsed = parse(&good).unwrap();
        assert!(
            error(&parsed, Policy::Command)
                .unwrap()
                .unwrap()
                .error
                .input()
                .is_some()
        );
        assert!(error(&parsed, Policy::Observation).is_err());
        for raw in [
            format!(r#"{{"error":{input},"error":{input}}}"#),
            format!(r#"{{"error":{input},"request_id":null,"request_id":"retry"}}"#),
            format!(r#"{{"error":{input},"extra":null}}"#),
            r#"{"error":{"code":"provider_failed","code":"provider_failed","message":"x"}}"#.into(),
            r#"{"error":{"code":"provider_failed","message":"first","message":"second"}}"#.into(),
            r#"{"error":["provider_failed","x"]}"#.into(),
            r#"{"error":{"code":"out_of_range","message":"x"}}"#.into(),
        ] {
            assert_eq!(
                parse(&raw).unwrap_err().code(),
                "daemon_protocol_error",
                "{raw}"
            );
        }
        // Successful payload decoding is outside this error-transport contract.
        assert_eq!(parse(r#"{"value":1,"value":2}"#).unwrap()["value"], 2);
    }
}
