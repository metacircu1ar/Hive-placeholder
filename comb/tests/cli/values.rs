use super::*;

#[test]
fn identifier_conversions_keep_cli_json_errors_and_date_context() {
    let dir = tempfile::tempdir().unwrap();
    cli(dir.path(), &["init"], None, true).unwrap();
    for (args, code) in [
        (vec!["get", "../bad"], "invalid_value"),
        (
            vec!["capture", "Text", "--request-id", ""],
            "invalid_request_id",
        ),
        (vec!["--run", "\n", "capture", "Text"], "invalid_run_name"),
        (
            vec!["search", "text", "--after", "not-a-date"],
            "invalid_value",
        ),
        (
            vec!["search", "text", "--before", "not-a-date"],
            "invalid_value",
        ),
    ] {
        let output = cli(dir.path(), &args, None, false).unwrap();
        assert_eq!(output["error"]["code"], code);
        assert!(output["error"].get("request_id").is_none());
    }
}

#[test]
fn malformed_expected_hash_is_a_caller_error_before_reconciliation() {
    let directory = tempfile::tempdir().unwrap();
    cli(directory.path(), &["init"], None, true).unwrap();
    let page = "---\ntitle: Hash validation\ntype: component\n---\nOriginal\n";
    cli(
        directory.path(),
        &["put", "concepts/hash", "--request-id", "original"],
        Some(page),
        true,
    )
    .unwrap();
    let path = directory.path().join("concepts/hash.md");
    std::fs::write(&path, "unparseable external page").unwrap();
    for hash in [String::new(), "malformed".into(), "A".repeat(64)] {
        let output = Command::new(env!("CARGO_BIN_EXE_comb"))
            .arg("--root")
            .arg(directory.path())
            .args(["rm", "concepts/hash", "--expected-hash", &hash])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reply["error"]["code"], "invalid_value");
        assert_eq!(
            reply["error"]["expected"],
            "64 lowercase hexadecimal characters"
        );
        assert_eq!(reply["error"]["field"], "--expected-hash");
        assert!(reply["error"].get("request_id").is_none());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "unparseable external page"
        );
    }
}

#[test]
fn search_date_errors_identify_the_field_without_echoing_input() {
    let directory = tempfile::tempdir().unwrap();
    for filter in ["--after", "--before"] {
        let output = Command::new(env!("CARGO_BIN_EXE_comb"))
            .arg("--root")
            .arg(directory.path())
            .args(["search", "text", filter, "PRIVATE-invalid-date"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["code"], "invalid_value");
        assert_eq!(value["error"]["field"], filter);
        assert_eq!(
            value["error"]["expected"],
            "an RFC3339 timestamp with an explicit timezone"
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE"));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}
