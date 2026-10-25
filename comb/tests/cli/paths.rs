//! Selected-path rejection must not create state, issue credentials or connect.
use super::*;
use std::{
    fs,
    os::unix::{
        fs::{PermissionsExt, symlink},
        net::UnixListener,
    },
    process::Output,
};

fn invoke(root: &Path, args: &[&str]) -> TestResult<Output> {
    Ok(Command::new(env!("CARGO_BIN_EXE_comb"))
        .arg("--root")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()?)
}
fn rejected(output: &Output, field: &str) -> TestResult<()> {
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reply: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(reply["error"]["code"], "invalid_path");
    assert_eq!(reply["error"]["field"], field);
    assert!(reply["error"]["examples"].is_array());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE"));
    Ok(())
}

#[test]
fn missing_file_and_uninitialized_roots_are_actionable_without_initializing_them() -> TestResult<()>
{
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let root = dir.path().join("PRIVATE-root");
    for kind in ["missing", "file", "directory"] {
        match kind {
            "file" => fs::write(&root, "UNCHANGED").unwrap(),
            "directory" => {
                fs::remove_file(&root).unwrap();
                fs::create_dir(&root).unwrap();
            }
            _ => (),
        }
        for args in [
            vec!["stats"],
            vec!["get", "concepts/one"],
            vec!["serve"],
            vec!["sync", "--rebuild"],
        ] {
            let output = invoke(&root, &args)?;
            rejected(&output, "--root")?;
            if kind != "file" {
                assert!(String::from_utf8_lossy(&output.stdout).contains("comb init DIR"));
            }
        }
        assert!(!root.join(".comb").exists());
        if kind == "missing" {
            assert!(!root.exists());
        }
        if kind == "file" {
            assert_eq!(fs::read_to_string(&root).unwrap(), "UNCHANGED");
        }
        if kind == "directory" {
            assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        }
    }
    Ok(())
}

#[test]
fn symlinks_and_corrupt_administration_stay_operational_not_init_guidance() -> TestResult<()> {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    for kind in [
        "root-link",
        "admin-link",
        "admin-file",
        "admin-mode",
        "identity",
    ] {
        let root = dir.path().join(kind);
        if kind == "root-link" {
            symlink(dir.path().join("missing"), &root).unwrap();
        } else {
            drop(comb::Comb::init(&root).unwrap());
            let admin = root.join(".comb");
            match kind {
                "admin-link" | "admin-file" => {
                    fs::rename(&admin, root.join("retained-admin")).unwrap();
                    if kind == "admin-link" {
                        symlink(root.join("missing"), &admin).unwrap();
                    } else {
                        fs::write(&admin, "UNCHANGED").unwrap();
                    }
                }
                "admin-mode" => {
                    fs::set_permissions(&admin, fs::Permissions::from_mode(0o755)).unwrap();
                }
                "identity" => fs::write(admin.join("identity.json"), "PRIVATE_CORRUPTION").unwrap(),
                _ => unreachable!(),
            }
        }
        let output = invoke(&root, &["stats"])?;
        assert_eq!(output.status.code(), Some(1), "{kind}");
        let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_ne!(reply["error"]["code"], "invalid_path");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("comb init"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("PRIVATE"));
        if kind == "admin-file" {
            assert_eq!(fs::read(root.join(".comb")).unwrap(), b"UNCHANGED");
        }
        if kind == "identity" {
            assert_eq!(
                fs::read(root.join(".comb/identity.json")).unwrap(),
                b"PRIVATE_CORRUPTION"
            );
        }
        if kind.ends_with("link") {
            let link = if kind == "root-link" {
                root
            } else {
                root.join(".comb")
            };
            assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
        }
    }
    Ok(())
}

#[test]
fn invalid_remote_socket_and_root_cancel_reserved_output_without_issuing() -> TestResult<()> {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let root = dir.path().join("knowledge");
    let comb = comb::Comb::init(&root).unwrap();
    let index = root.join(".comb/index.sqlite");
    let before = fs::read(&index).unwrap();
    let entries = || {
        let mut names = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        names.sort();
        names
    };
    let before_entries = entries();
    let out = dir.path().join("client.json");
    let invalid_socket = dir.path().join("missing/service.sock");
    for socket in [invalid_socket.to_str().unwrap().to_owned(), "x".repeat(300)] {
        for args in [
            vec!["--socket", &socket, "stats"],
            vec![
                "--socket",
                &socket,
                "credential",
                "create",
                "reader",
                "--out",
                out.to_str().unwrap(),
            ],
        ] {
            rejected(&invoke(&root, &args)?, "--socket")?;
            assert!(!out.exists());
            assert_eq!(fs::read(&index).unwrap(), before);
            assert_eq!(entries(), before_entries);
        }
    }
    rejected(
        &invoke(
            &dir.path().join("absent"),
            &[
                "credential",
                "create",
                "reader",
                "--out",
                out.to_str().unwrap(),
            ],
        )?,
        "--root",
    )?;
    assert!(!out.exists());
    assert_eq!(entries(), before_entries);
    assert_eq!(comb.stats().unwrap().pages, 0);
    Ok(())
}

#[test]
fn invalid_root_and_payload_never_contact_an_explicit_live_socket() -> TestResult<()> {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let socket = dir.path().join("sentinel.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let root = dir.path().join("absent");
    rejected(
        &invoke(&root, &["--socket", socket.to_str().unwrap(), "stats"])?,
        "--root",
    )?;
    let output = invoke(
        &root,
        &[
            "--socket",
            socket.to_str().unwrap(),
            "put",
            "concepts/one",
            "--file",
            "PRIVATE-missing.md",
        ],
    )?;
    assert_eq!(output.status.code(), Some(2));
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reply["error"]["field"], "--file");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(!root.exists());
    Ok(())
}

#[test]
fn mcp_credential_file_failures_are_bounded_private_and_protocol_silent() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let path = dir.path().join("PRIVATE-credential.json");
    for content in [
        None,
        Some(b"PRIVATE_PAYLOAD".to_vec()),
        Some(vec![b'x'; 16385]),
    ] {
        if let Some(bytes) = content {
            fs::write(&path, bytes).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let output = Command::new(env!("CARGO_BIN_EXE_comb"))
            .args(["mcp", "--credential"])
            .arg(&path)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let text = String::from_utf8(output.stderr).unwrap();
        assert!(text.contains("--credential"));
        assert!(text.contains("comb mcp --credential"));
        assert!(!text.contains("PRIVATE"));
    }
}

#[test]
fn mcp_socket_override_is_diagnosed_after_valid_credential_preparation() {
    use comb::service::auth::{Connection, ConnectionExt as _};
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let root = dir.path().join("knowledge");
    let comb = comb::Comb::init(&root).unwrap();
    let path = dir.path().join("client.json");
    Connection::human(&root, dir.path().join("unused.sock"))
        .unwrap()
        .write(&path)
        .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_comb"))
        .args(["--socket", &"x".repeat(300), "mcp", "--credential"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let text = String::from_utf8(output.stderr).unwrap();
    assert!(text.contains("--socket"));
    assert!(text.contains("invalid_path"));
    assert_eq!(comb.stats().unwrap().pages, 0);
}

#[test]
fn group_writable_root_and_namespace_support_cli_writes_rebuild_and_cleanup() -> TestResult<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    fs::set_permissions(root, fs::Permissions::from_mode(0o775))?;
    if rustix::process::geteuid().is_root() {
        assert!(!invoke(root, &["init"])?.status.success());
        return Ok(());
    }
    cli(root, &["init"], None, true)?;
    let namespace = root.join("projects");
    fs::create_dir(&namespace)?;
    fs::set_permissions(&namespace, fs::Permissions::from_mode(0o775))?;
    let body = "---\ntitle: Example\ntype: component\n---\nGroup directory fixture.\n";
    cli(root, &["put", "projects/example"], Some(body), true)?;
    let before = cli(root, &["get", "projects/example"], None, true)?;
    let abandoned = root.join(".comb/.comb-tmp-admin-ABC123");
    fs::write(&abandoned, b"abandoned preparation")?;
    fs::set_permissions(&abandoned, fs::Permissions::from_mode(0o600))?;
    cli(root, &["stats"], None, true)?;
    assert!(!abandoned.exists());
    cli(root, &["sync", "--rebuild"], None, true)?;
    let after = cli(root, &["get", "projects/example"], None, true)?;
    assert_eq!(before["hash"], after["hash"]);
    for path in [root, &namespace] {
        assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, 0o775);
    }
    let admin = root.join(".comb");
    assert_eq!(fs::metadata(&admin)?.permissions().mode() & 0o777, 0o700);
    fs::set_permissions(&admin, fs::Permissions::from_mode(0o770))?;
    assert!(!invoke(root, &["stats"])?.status.success());
    fs::set_permissions(&admin, fs::Permissions::from_mode(0o700))?;
    fs::set_permissions(&namespace, fs::Permissions::from_mode(0o777))?;
    assert!(!invoke(root, &["get", "projects/example"])?.status.success());
    Ok(())
}
