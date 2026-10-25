//! Strict offline replacement flags must fail before recovery or backfill.
use super::TestResult;
use comb::Comb;
use serde_json::{Value, json};
use std::{path::Path, process::Command};

fn config(root: &Path, revision: &str) -> TestResult<std::path::PathBuf> {
    let path = root.join(format!("{revision}.json"));
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS).write(
        &path,
        &serde_json::to_vec(&json!({"backend":"http","model":"PRIVATE_MODEL",
            "revision":revision,"dimensions":2,"url":"http://127.0.0.1:9/v1/embeddings"}))?,
    )?;
    Ok(path)
}
fn run(root: &Path, config: &Path, replace: bool) -> TestResult<std::process::Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_comb"));
    command
        .arg("--root")
        .arg(root)
        .args(["embed", "--config"])
        .arg(config);
    if replace {
        command.arg("--replace");
    }
    Ok(command.output()?)
}
fn rejected(output: std::process::Output, code: &str) -> TestResult<()> {
    assert_eq!(output.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(error["error"]["field"], "--replace");
    assert_eq!(error["error"]["code"], code);
    assert!(
        error["error"]["examples"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE_MODEL"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE_MODEL"));
    Ok(())
}
#[test]
fn redundant_embed_replacement_is_rejected_on_first_and_same_activation() -> TestResult<()> {
    let directory = tempfile::tempdir_in("/tmp")?;
    drop(Comb::init(directory.path())?);
    let config = config(directory.path(), "pin")?;
    rejected(
        run(directory.path(), &config, true)?,
        "conflicting_arguments",
    )?;
    assert!(run(directory.path(), &config, false)?.status.success());
    rejected(
        run(directory.path(), &config, true)?,
        "conflicting_arguments",
    )?;
    assert!(run(directory.path(), &config, false)?.status.success());
    Ok(())
}
#[test]
fn changed_embed_space_requires_replacement_and_retains_population_on_rejection() -> TestResult<()>
{
    let directory = tempfile::tempdir_in("/tmp")?;
    drop(Comb::init(directory.path())?);
    let first = config(directory.path(), "first")?;
    let second = config(directory.path(), "second")?;
    assert!(run(directory.path(), &first, false)?.status.success());
    let marker = directory.path().join(".comb/index-population.json");
    let bytes = b"{\"operation\":\"populate_index\",\"space\":null}\n";
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(&marker, bytes)?;
    rejected(run(directory.path(), &second, false)?, "missing_argument")?;
    assert_eq!(std::fs::read(&marker)?, bytes);
    assert!(run(directory.path(), &second, true)?.status.success());
    assert!(!marker.exists());
    Ok(())
}
