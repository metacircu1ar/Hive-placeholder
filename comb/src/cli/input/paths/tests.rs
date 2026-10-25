use super::*;
use crate::cli::{
    Cli,
    command::{Command, LocalCommand},
};
use clap::Parser;
use comb::service::auth::{ConnectionExt as _, Credential};
use std::os::unix::fs::PermissionsExt;

#[test]
fn prepared_mcp_connection_survives_removing_its_source() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let path = dir.path().join("client.json");
    let connection = Connection {
        format: hive_input::compatibility::Exact,
        socket: dir.path().join("unused.sock"),
        credential: Credential {
            comb_id: comb::values::CombId::generate(),
            id: comb::values::CredentialId::generate(),
            secret: "PRIVATE_SECRET".into(),
        },
    };
    connection.write(&path).unwrap();
    let cli = Cli::try_parse_from([
        std::ffi::OsStr::new("comb"),
        "mcp".as_ref(),
        "--credential".as_ref(),
        path.as_os_str(),
    ])
    .unwrap();
    assert!(cli.is_mcp());
    let Command::Local(LocalCommand::Mcp {
        connection: prepared,
    }) = Command::try_from(cli.command).unwrap()
    else {
        panic!("MCP command")
    };
    fs::remove_file(&path).unwrap();
    assert_eq!(prepared.credential.id, connection.credential.id);
    assert_eq!(prepared.credential.secret, "PRIVATE_SECRET");
    assert_eq!(prepared.socket, connection.socket);
}

#[test]
fn credential_file_shapes_and_private_mode_are_enforced_at_read_boundary() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let path = dir.path().join("PRIVATE-client.json");
    let id = comb::values::CombId::generate();
    let credential = comb::values::CredentialId::generate();
    let valid = serde_json::json!({"format":1,"socket": "/tmp/unused.sock", "credential": {"comb_id": id, "id": credential, "secret": "PRIVATE_SECRET"}});
    let text = valid.to_string();
    let invalid = [
        serde_json::json!([valid["socket"], valid["credential"]]).to_string(),
        serde_json::json!({"format":1,"socket":"/tmp/unused.sock", "credential": [id, credential, "PRIVATE_SECRET"]}).to_string(),
        text.replace("\"socket\":", "\"socket\":\"/tmp/duplicate.sock\",\"socket\":"),
        text.replace("\"secret\":", "\"secret\":\"duplicate\",\"secret\":"),
        text.replace("/tmp/unused.sock", "relative.sock"),
    ];
    for content in invalid {
        fs::write(&path, content).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let output = Context::new("mcp", "--credential")
            .credential(&path)
            .err()
            .unwrap()
            .render_mcp();
        assert_eq!(output.exit_code, 2);
        assert!(output.stderr.contains("--credential"));
        assert!(!output.stderr.contains("PRIVATE"));
        assert!(output.stdout.is_empty());
    }
    fs::write(&path, text).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        Context::new("mcp", "--credential")
            .credential(&path)
            .is_ok()
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        Context::new("mcp", "--credential")
            .credential(&path)
            .is_err()
    );
}

#[test]
fn cleanup_failure_preserves_both_causes_and_native_resource_errors_stay_operational() {
    let selected = Context::new("stats", "--root").file(Path::new("PRIVATE_ROOT"));
    let input = selected.invalid(Code::InvalidPath, "an initialized Comb root");
    let cleaned = input.with_cleanup(Err(Error::from(io::Error::other("CLEANUP_SENTINEL"))));
    let output = cleaned.render();
    assert_eq!(output.exit_code, 1);
    assert!(output.stderr.contains("CLEANUP_SENTINEL"));
    assert!(output.stderr.contains("--root"));
    assert!(!output.stdout.contains("CLEANUP_SENTINEL"));
    let output = selected
        .root_io(
            io::Error::from_raw_os_error(libc::EIO),
            Path::new("PRIVATE_ROOT"),
        )
        .render();
    assert_eq!(output.exit_code, 1);
    assert!(output.stdout.contains("io_error"));
    assert!(output.stderr.contains("Path: PRIVATE_ROOT"));
    let output = selected
        .invalid(Code::InvalidPath, "an initialized Comb root")
        .with_cleanup(Ok(()))
        .render();
    assert_eq!(output.exit_code, 2);
}
