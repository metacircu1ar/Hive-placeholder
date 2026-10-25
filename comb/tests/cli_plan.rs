#![allow(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact the reviewed maintainer write inventory inventory"
)]
//! Lifecycle and fallback checks for the typed CLI plan.
use comb::service::auth::ConnectionExt as _;
use comb::{
    Comb,
    service::auth::{Connection, Credential},
    values::{CombId, CredentialId},
};
use std::process::{Command, Stdio};

#[test]
fn mcp_eof_succeeds_without_an_extra_json_line() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("client.json");
    Connection {
        format: hive_input::compatibility::Exact,
        socket: directory.path().join("unused.sock"),
        credential: Credential {
            comb_id: CombId::generate(),
            id: CredentialId::generate(),
            secret: "a".repeat(64),
        },
    }
    .write(&path)
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_comb"))
        .args(["mcp", "--credential"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    let diagnostics = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        diagnostics
            .lines()
            .map(|line| line.split_whitespace().nth(2).unwrap())
            .collect::<Vec<_>>(),
        ["comb.mcp_started", "comb.mcp_ended"]
    );
    assert!(!diagnostics.contains(&"a".repeat(64)));
    assert!(!diagnostics.contains(directory.path().to_str().unwrap()));
}

#[test]
fn mcp_reads_credential_before_resolving_an_invalid_socket() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_comb"))
        .args(["--socket", &"x".repeat(300), "mcp", "--credential"])
        .arg(directory.path().join("missing.json"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostic.contains("--credential"));
    assert!(diagnostic.contains("invalid_file"));
    assert!(!diagnostic.contains("--socket"));
}

#[test]
fn offline_search_validates_embedding_configuration_before_opening_comb() {
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let comb = Comb::init(directory.path()).unwrap();
    let config = directory.path().join("absent.yaml");
    let output = Command::new(env!("CARGO_BIN_EXE_comb"))
        .arg("--root")
        .arg(directory.path())
        .args(["search", "text", "--embedding-config"])
        .arg(config)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let reply: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["error"]["code"], "invalid_file");
    assert!(reply["error"].get("request_id").is_none());
    assert_eq!(reply["error"]["field"], "--embedding-config");
    assert!(reply["error"]["examples"].is_array());
    assert_eq!(comb.stats().unwrap().pages, 0);
}

#[cfg(test)]
#[path = "cli_plan/import.rs"]
mod import;

#[cfg(test)]
#[path = "cli_plan/socket.rs"]
mod socket;
