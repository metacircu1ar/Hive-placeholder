//! Pure config errors precede root and managed-service discovery.
use super::TestResult;
use serde_json::{Value, json};
use std::{path::Path, process::Command};

fn http() -> Value {
    json!({"backend":"http","model":"PRIVATE_MODEL","revision":"pin",
        "dimensions":2,"url":"http://127.0.0.1:9/v1/embeddings"})
}
fn invalid(root: &Path, path: &Path, config: &Value, property: &str) -> TestResult<()> {
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(path, &serde_json::to_vec(config)?)?;
    for (args, flag) in [
        (vec!["embed", "--config"], "--config"),
        (vec!["serve", "--embedding-config"], "--embedding-config"),
        (
            vec!["search", "PRIVATE_QUERY", "--embedding-config"],
            "--embedding-config",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_comb"))
            .arg("--root")
            .arg(root)
            .args(args)
            .arg(path)
            .output()?;
        assert_eq!(output.status.code(), Some(2));
        let reply: Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(reply["error"]["field"], flag);
        assert!(
            reply["error"]["expected"]
                .as_str()
                .is_some_and(|s| s.contains(property))
        );
        assert!(
            reply["error"]["examples"]
                .as_array()
                .is_some_and(|a| !a.is_empty())
        );
        for private in ["PRIVATE_MODEL", "PRIVATE_QUERY", "PRIVATE_URL"] {
            assert!(!String::from_utf8_lossy(&output.stdout).contains(private));
            assert!(!String::from_utf8_lossy(&output.stderr).contains(private));
        }
        assert_eq!(std::fs::read(root)?, b"untouched");
    }
    Ok(())
}
#[test]
fn http_scalar_errors_name_the_configuration_before_root_inspection() -> TestResult<()> {
    let directory = tempfile::tempdir_in("/tmp")?;
    let root = directory.path().join("poisoned-root");
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(&root, b"untouched")?;
    let path = directory.path().join("config.json");
    for (field, value) in [
        (
            "url",
            json!("https://PRIVATE_URL.invalid/?secret=PRIVATE_URL"),
        ),
        ("model", json!("")),
        ("revision", json!("")),
        ("dimensions", json!(0)),
        ("timeout_seconds", json!(0)),
        ("query_prefix", json!("x".repeat(1025))),
        ("document_prefix", json!("x".repeat(1025))),
        ("max_tokens", json!(16)),
    ] {
        let mut config = http();
        config[field] = value;
        invalid(&root, &path, &config, field)?;
    }
    let mut config = http();
    config["tokenizer"] = json!("missing-tokenizer");
    invalid(&root, &path, &config, "max_tokens")?;
    config["max_tokens"] = json!(15);
    invalid(&root, &path, &config, "max_tokens")?;
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 2);
    Ok(())
}
#[test]
fn the_removed_local_backend_is_refused_before_root_inspection() -> TestResult<()> {
    let directory = tempfile::tempdir_in("/tmp")?;
    let root = directory.path().join("poisoned-root");
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(&root, b"untouched")?;
    let path = directory.path().join("config.json");
    let local = json!({"backend":"local","model":"PRIVATE_MODEL","directory":"missing-assets",
        "dimensions":2,"threads":2});
    invalid(&root, &path, &local, "valid embedding configuration")?;
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 2);
    Ok(())
}
#[test]
fn invalid_configuration_precedes_managed_record_discovery() -> TestResult<()> {
    let directory = tempfile::tempdir_in("/tmp")?;
    let path = directory.path().join("config.json");
    let mut config = http();
    config["dimensions"] = json!(0);
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(&path, &serde_json::to_vec(&config)?)?;
    let output = Command::new(env!("CARGO_BIN_EXE_comb"))
        .arg("--root")
        .arg(directory.path().join("absent"))
        .arg("--managed-service")
        .arg(directory.path().join("absent-record"))
        .args(["serve", "--embedding-config"])
        .arg(path)
        .output()?;
    assert_eq!(output.status.code(), Some(2));
    let reply: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(reply["error"]["field"], "--embedding-config");
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
    Ok(())
}
