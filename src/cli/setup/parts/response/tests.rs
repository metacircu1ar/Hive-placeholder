use super::*;
use std::error::Error as _;

#[test]
fn forwarded_rejection_uses_only_trusted_metadata_and_retains_private_cause() {
    let stdout = serde_json::json!({"error": {"code":"invalid_value", "message":"PRIVATE message",
        "field":"--embedding-config", "expected":"PRIVATE expected", "examples":["PRIVATE example"]}})
    .to_string();
    let error = decode::<serde_json::Value>(
        &["--embedding-config"],
        Output {
            code: 2,
            stdout,
            stderr: "PRIVATE stderr".into(),
        },
    )
    .unwrap_err();
    let Error::Input { input, .. } = &error else {
        panic!("typed input");
    };
    assert_eq!(input.field(), "--embedding-config");
    assert!(!serde_json::to_string(&error).unwrap().contains("PRIVATE"));
    assert!(error.source().unwrap().to_string().contains("PRIVATE"));
}

#[test]
fn malformed_unknown_and_operational_child_errors_do_not_become_input_or_echo() {
    for (code, stdout) in [
        (2, "PRIVATE invalid json".to_owned()),
        (1, "PRIVATE native failure".to_owned()),
        (0, "PRIVATE invalid success".to_owned()),
        (
            2,
            serde_json::json!({"error": {"code":"invalid_value", "message":"PRIVATE",
            "field":"--unforwarded", "expected":"PRIVATE", "examples":["PRIVATE"]}})
            .to_string(),
        ),
    ] {
        let error = decode::<serde_json::Value>(
            &[],
            Output {
                code,
                stdout,
                stderr: "PRIVATE".into(),
            },
        )
        .unwrap_err();
        assert!(!matches!(error, Error::Input { .. }));
        assert!(!serde_json::to_string(&error).unwrap().contains("PRIVATE"));
        assert!(error.source().is_some());
    }
}
