//! Invalid credential output is rejected before issuance or knowledge access.
use std::{
    os::unix::{
        ffi::OsStringExt,
        fs::{PermissionsExt, symlink},
        net::UnixListener,
    },
    path::Path,
    process::Command,
};
type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn rejected(root: &Path, socket: &Path, output: &Path) -> Result {
    let result = Command::new(env!("CARGO_BIN_EXE_comb"))
        .arg("--root")
        .arg(root)
        .arg("--socket")
        .arg(socket)
        .args(["credential", "create", "client", "--out"])
        .arg(output)
        .output()?;
    assert_eq!(
        result.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let error: serde_json::Value = serde_json::from_slice(&result.stdout)?;
    assert_eq!(error["error"]["field"], "--out");
    assert_eq!(error["error"]["code"], "invalid_path");
    assert!(!String::from_utf8_lossy(&result.stdout).contains("PRIVATE"));
    assert!(!result.stderr.contains(&0x1b));
    Ok(())
}
#[test]
fn bad_output_paths_never_reach_the_store_or_sentinel_socket() -> Result {
    let dir = tempfile::tempdir_in("/tmp")?;
    let root = dir.path().join("absent");
    let socket = dir.path().join("sentinel.sock");
    let listener = UnixListener::bind(&socket)?;
    listener.set_nonblocking(true)?;
    let file = dir.path().join("PRIVATE-file");
    std::fs::write(&file, "keep")?;
    let link = dir.path().join("PRIVATE-link");
    symlink(dir.path().join("missing"), &link)?;
    let shared = dir.path().join("shared");
    std::fs::create_dir(&shared)?;
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777))?;
    let parent_link = dir.path().join("parent-link");
    symlink(dir.path(), &parent_link)?;
    let paths = [
        dir.path().join(".hive-tmp-state-ABC123"),
        dir.path().join(".hive-tmp-state-0123456789abcdef"),
        dir.path().join(".comb-tmp-admin-ABC123"),
        dir.path().join(".comb-tmp-admin-0123456789abcdef"),
        file.clone(),
        link,
        dir.path().join("PRIVATE\n\x1b/missing"),
        shared.join("PRIVATE-out"),
        parent_link.join("PRIVATE-out"),
        std::path::PathBuf::from(std::ffi::OsString::from_vec(b"PRIVATE-\xff".to_vec())),
    ];
    for path in paths {
        rejected(&root, &socket, &path)?;
    }
    assert_eq!(std::fs::read(&file)?, b"keep");
    assert!(!root.exists());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    Ok(())
}
#[test]
fn unwritable_parent_is_a_caller_error_before_issuance() -> Result {
    if rustix::process::geteuid().is_root() {
        return Ok(());
    }
    let dir = tempfile::tempdir_in("/tmp")?;
    let parent = dir.path().join("parent");
    std::fs::create_dir(&parent)?;
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o500))?;
    let root = dir.path().join("missing");
    let result = rejected(
        &root,
        &dir.path().join("missing.sock"),
        &parent.join("PRIVATE-out"),
    );
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700))?;
    result?;
    assert!(!root.exists());
    assert_eq!(std::fs::read_dir(parent)?.count(), 0);
    Ok(())
}

#[test]
fn reserved_server_socket_is_rejected_before_root_access() -> Result {
    for name in [".hive-tmp-state-ABC123", ".comb-tmp-admin-ABC123"] {
        let dir = tempfile::tempdir_in("/tmp")?;
        let root = dir.path().join("absent");
        let socket = dir.path().join(name);
        let result = Command::new(env!("CARGO_BIN_EXE_comb"))
            .arg("--root")
            .arg(&root)
            .arg("--socket")
            .arg(&socket)
            .arg("serve")
            .output()?;
        assert_eq!(result.status.code(), Some(2));
        let error: serde_json::Value = serde_json::from_slice(&result.stdout)?;
        assert_eq!(error["error"]["field"], "--socket");
        assert_eq!(error["error"]["code"], "invalid_path");
        assert_eq!(std::fs::read_dir(dir.path())?.count(), 0);
    }

    Ok(())
}
