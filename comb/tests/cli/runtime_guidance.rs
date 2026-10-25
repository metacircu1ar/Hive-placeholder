//! Runtime advice must match exclusive commands without changing native outcomes.
use super::TestResult;
use comb::{Comb, Error};
use serde_json::{Value, json};
use std::process::Command;

#[test]
fn exclusive_commands_explain_the_same_root_stop_and_preserve_busy_errors() -> TestResult<()> {
    let directory = tempfile::tempdir_in("/tmp")?;
    let root = directory.path().join("knowledge");
    let comb = Comb::init(&root)?;
    let config = directory.path().join("embedding.json");
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS).write(
        &config,
        &serde_json::to_vec(&json!({"backend":"http","model":"tiny",
        "revision":"pin","dimensions":2,"url":"http://127.0.0.1:9/v1/embeddings"}))?,
    )?;
    let index = root.join(".comb/index.sqlite");
    let before = std::fs::read(&index)?;
    for args in [
        vec!["init"],
        vec!["embed", "--config"],
        vec!["sync", "--rebuild"],
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_comb"));
        command.arg("--root").arg(&root).args(&args);
        if args[0] == "embed" {
            command.arg(&config);
        }
        let output = command.output()?;
        assert_eq!(output.status.code(), Some(1));
        let reply: Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(reply["error"], serde_json::to_value(Error::already_open())?);
        let diagnostic = String::from_utf8(output.stderr)?;
        assert!(diagnostic.contains("exclusive access"));
        assert!(diagnostic.contains("comb --root ROOT service stop"));
        assert!(diagnostic.contains("same root"));
        assert!(diagnostic.contains("Examples:"));
        assert!(diagnostic.contains(args[0]));
        assert!(!diagnostic.contains("use its serving process"));
        assert_eq!(std::fs::read(&index)?, before);
        assert_eq!(comb.stats()?.pages, 0);
    }
    Ok(())
}
#[test]
fn missing_query_model_explains_configuration_without_changing_its_wire_error() -> TestResult<()> {
    let directory = tempfile::tempdir_in("/tmp")?;
    drop(Comb::init(directory.path())?);
    let output = Command::new(env!("CARGO_BIN_EXE_comb"))
        .arg("--root")
        .arg(directory.path())
        .args(["search", "question", "--mode", "vector"])
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    let reply: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        reply["error"],
        serde_json::to_value(Error::query_model_missing())?
    );
    let diagnostic = String::from_utf8(output.stderr)?;
    assert!(diagnostic.contains("--embedding-config"));
    assert!(diagnostic.contains("--mode text"));
    assert!(diagnostic.contains("Examples:"));
    Ok(())
}
