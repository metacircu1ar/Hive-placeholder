#![allow(clippy::unwrap_used)] // Private batch fixtures fail loudly on unexpected results.
use super::*;
use serde_json::json;

#[test]
fn batch_source_is_read_once_and_remote_pointer_keeps_private_path_local() {
    let failure = json!({"request_id":"outer","error":{"code":"invalid_value","message":"Invalid batch arguments.",
        "field":"/operations/0/target","expected":"an agent target","examples":[r#"{"op":"batch","operations":[]}"#]}});
    let stub = response_tests::Stub::new(vec![failure]);
    let path = stub.root().join("PRIVATE\noperations.json");
    std::fs::write(
        &path,
        r#"[{"op":"send","request_id":"inner","target":"a","body":"PRIVATE_BODY","extra":42}]"#,
    )
    .unwrap();
    let cli = Cli::try_parse_from([
        "hivectl",
        "--config",
        "/unused",
        "--state-dir",
        stub.root().to_str().unwrap(),
        "batch",
        "--operations-file",
        path.to_str().unwrap(),
        "--request-id",
        "outer",
    ])
    .unwrap();
    let prepared = dispatch::Prepared::new(cli).unwrap();
    std::fs::remove_file(&path).unwrap();
    let outcome = dispatch::execute(prepared).unwrap();
    assert!(!outcome.success);
    assert_eq!(outcome.value["request_id"], "outer");
    assert_eq!(outcome.value["error"]["field"], "--operations-file");
    assert!(
        outcome.value["error"]["expected"]
            .as_str()
            .unwrap()
            .starts_with("/operations/0/target:")
    );
    assert!(!outcome.value.to_string().contains("PRIVATE"));
    assert!(outcome.input.is_some());
    let wire = &stub.finish()[0]["args"];
    assert_eq!(wire["operations"][0]["extra"], 42);
    assert_eq!(wire["operations"][0]["body"], "PRIVATE_BODY");
    assert!(!wire.to_string().contains("operations.json"));
}

#[test]
fn batch_remote_context_rejects_noncanonical_or_unrecognized_pointers() {
    for (pointer, field) in [
        ("/operations", Some("--operations-file")),
        ("/operations/0", Some("--operations-file")),
        ("/operations/63/op", Some("--operations-file")),
        ("/operations/0/name", Some("--operations-file")),
        ("/join", Some("--join")),
        ("/join/deadline_seconds", Some("--deadline-seconds")),
        ("/operations/64/op", None),
        ("/operations/01/op", None),
        ("/operations/0/PRIVATE", None),
        ("/operations/0/request_id/extra", None),
        ("/join/PRIVATE", None),
    ] {
        assert_eq!(batch::field(pointer), field);
    }
    let error = json!({"error":{"code":"invalid_value","message":"Invalid batch.","field":"/operations/0/PRIVATE",
        "expected":"safe","examples":["hivectl batch --operations-file operations.json"]}});
    let stub = response_tests::Stub::new(vec![error]);
    assert_eq!(
        remote_response(
            stub.root(),
            "outer",
            Request::Batch(api::BatchArgs::default())
        )
        .err()
        .unwrap()
        .code(),
        "daemon_protocol_error"
    );
    stub.finish();
}

#[test]
fn batch_preparation_checks_final_envelope_not_only_source_file_size() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("operations.json");
    let mut operations = json!([{"op":"send","request_id":"inner","extra":""}]);
    let overhead = operations.to_string().len();
    operations[0]["extra"] = json!("x".repeat(BATCH_BYTES - overhead));
    let text = operations.to_string();
    assert_eq!(text.len(), BATCH_BYTES);
    std::fs::write(&path, text).unwrap();
    let cli = Cli::try_parse_from([
        "hivectl",
        "batch",
        "--operations-file",
        path.to_str().unwrap(),
    ])
    .unwrap();
    let Err(failure) = dispatch::Prepared::new(cli) else {
        panic!("envelope must exceed cap")
    };
    assert_eq!(failure.error.code(), "out_of_range");
    assert_eq!(
        serde_json::to_value(failure.error).unwrap()["field"],
        "--operations-file"
    );
}
