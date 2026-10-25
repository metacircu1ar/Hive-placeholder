use super::{parse, tests::rejected};
use std::ffi::OsString;

#[test]
fn model_use_accepts_only_none() {
    assert!(parse(&["comb", "model", "use", "none"].map(OsString::from)).is_ok());
    for name in ["", "default", "PRIVATE/name", "x"] {
        let output = rejected(&["model", "use", name]);
        assert_eq!(output.exit_code, 2);
        let json: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(json["error"]["field"], "NAME");
        assert_eq!(json["error"]["code"], "invalid_value");
        let expected = json["error"]["expected"].as_str().unwrap();
        assert!(
            expected.starts_with("none, the only accepted name"),
            "{expected}"
        );
        assert!(!json["error"]["examples"].as_array().unwrap().is_empty());
        assert!(!output.stdout.contains("PRIVATE"));
        assert!(!output.stderr.contains("PRIVATE"));
    }
}
