use super::*;
use serde_json::json;
use std::error::Error as _;

#[test]
fn raw_peer_codes_are_bounded_and_reserved_for_unknown_errors() {
    let original = json!({"code":"unknown_error","raw_code":"future_failure",
        "message":"Future failure.","file_committed":true});
    let error: Error = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(error.raw_code(), Some("future_failure"));
    assert_eq!(serde_json::to_value(error).unwrap(), original);
    for (key, value) in [
        ("code", json!("not_found")),
        ("code", json!("future_failure")),
        ("raw_code", json!(null)),
        ("raw_code", json!("")),
        ("raw_code", json!("not_found")),
        ("raw_code", json!("unknown_error")),
        ("raw_code", json!("private\ntext")),
        ("raw_code", json!("a".repeat(129))),
    ] {
        let mut malformed = original.clone();
        malformed[key] = value;
        assert!(serde_json::from_value::<Error>(malformed).is_err());
    }
}

#[test]
fn input_roundtrip_is_private_and_preserves_commit_and_request_metadata() {
    for code in [
        Code::InvalidJson,
        Code::InvalidRequestId,
        Code::InvalidValue,
    ] {
        let input = InputError::new(
            code,
            "Invalid input.",
            "/arguments/query",
            "nonblank text",
            vec!["comb_search {\"query\":\"architecture\"}".into()],
        )
        .unwrap()
        .with_public_received("private-marker");
        let mut error = Error::from(input)
            .committed()
            .caused_by(std::io::Error::other("private-cause"));
        error.request_id = Some("retained-request".into());
        let error = error.clone();
        assert_eq!(error.source().unwrap().to_string(), "private-cause");
        let wire = serde_json::to_string(&error).unwrap();
        assert!(!wire.contains("private"));
        let roundtrip: Error = serde_json::from_str(&wire).unwrap();
        assert_eq!(roundtrip.input().unwrap().code(), code);
        assert_eq!(roundtrip.code, error.code);
        assert!(roundtrip.file_committed);
        assert_eq!(roundtrip.request_id.as_deref(), Some("retained-request"));
        assert_eq!(serde_json::to_string(&roundtrip).unwrap(), wire);
        assert!(roundtrip.source().is_none());
    }
}

#[test]
fn incomplete_forged_and_positional_errors_are_rejected() {
    let input = json!({"code":"invalid_value", "message":"Invalid input.", "file_committed":false,
        "field":"/arguments/query", "expected":"nonblank text", "examples":["comb_search {\"query\":\"architecture\"}"]});
    for field in ["field", "expected", "examples"] {
        let mut missing = input.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<Error>(missing).is_err());
        let mut null = input.clone();
        null[field] = serde_json::Value::Null;
        assert!(serde_json::from_value::<Error>(null).is_err());
    }
    for (field, value) in [
        ("received", json!("private")),
        ("source", json!("private")),
        ("examples", json!([])),
        ("code", json!("forbidden")),
        ("field", json!("bad\nfield")),
    ] {
        let mut bad = input.clone();
        bad[field] = value;
        assert!(serde_json::from_value::<Error>(bad).is_err());
    }
    assert!(
        serde_json::from_str::<Error>(r#"["invalid_json","Invalid JSON.",false,null]"#).is_err()
    );
    let encoded = input.to_string();
    for (field, value) in [
        ("code", "\"invalid_value\""),
        ("field", "\"/arguments/query\""),
        ("examples", "[]"),
        ("request_id", "null"),
    ] {
        let duplicate = format!("{{\"{field}\":{value},{}", &encoded[1..]);
        let duplicate = if field == "request_id" {
            duplicate.replacen('{', "{\"request_id\":null,", 1)
        } else {
            duplicate
        };
        assert!(
            serde_json::from_str::<Error>(&duplicate).is_err(),
            "{field}"
        );
    }
    for code in ["invalid_json", "invalid_request_id"] {
        let native: Error = serde_json::from_value(
            json!({"code":code,"message":"Native failure.","file_committed":false}),
        )
        .unwrap();
        assert!(native.input().is_none());
    }
}
