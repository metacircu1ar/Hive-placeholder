use super::*;
use crate::config::{Method, Provider};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

fn profile(path: &Path) -> Profile {
    Profile {
        provider: Provider::Codex,
        method: Method::ApiKey,
        api_key_env: None,
        api_key_file: Some(path.into()),
    }
}
fn write(path: &Path, bytes: &[u8], mode: u32) {
    if path.exists() {
        fs::remove_file(path).unwrap();
    }
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn private_key_reads_are_bounded_and_do_not_follow_links() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let path = root.join("key");
    let selected = profile(&path);
    for value in [b"canary".as_slice(), b"canary\n", b"canary\r\n"] {
        write(&path, value, 0o400);
        assert_eq!(resolve(&selected).unwrap().0, "canary");
    }
    for (value, mode) in [
        (b"canary".as_slice(), 0o640),
        (b"canary", 0o700),
        (b"", 0o600),
        (b"canary\n\n", 0o600),
        (b"can ary", 0o600),
        (b"\xffcanary", 0o600),
        (b"can\0ary", 0o600),
    ] {
        write(&path, value, mode);
        assert!(resolve(&selected).is_err());
    }
    write(&path, &vec![b'a'; KEY_BYTES + 1], 0o600);
    assert!(resolve(&selected).is_err());
    write(&path, &vec![b'a'; KEY_BYTES], 0o600);
    assert_eq!(resolve(&selected).unwrap().0.len(), KEY_BYTES);
    let alias = root.join("alias");
    symlink(&path, &alias).unwrap();
    assert!(resolve(&profile(&alias)).is_err());
    let alias_dir = root.join("alias-dir");
    symlink(&root, &alias_dir).unwrap();
    assert!(resolve(&profile(&alias_dir.join("key"))).is_err());
    fs::remove_file(&alias).unwrap();
    fs::hard_link(&path, &alias).unwrap();
    assert!(resolve(&selected).is_err());
    fs::remove_file(&alias).unwrap();
    fs::remove_file(&path).unwrap();
    let native = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    // SAFETY: native is a live NUL-terminated path in an owned private test directory.
    assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
    assert!(resolve(&selected).is_err());
    assert!(resolve(&profile(&root)).is_err());
}

#[test]
fn key_file_ancestor_policy_and_atomic_replacement_are_preserved() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let dir = root.join("keys");
    fs::create_dir(&dir).unwrap();
    let path = dir.join("key");
    write(&path, b"old", 0o600);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o775)).unwrap();
    if os::effective_uid() != 0 {
        assert_eq!(resolve(&profile(&path)).unwrap().0, "old");
    }
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(resolve(&profile(&path)).is_err());
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(&path, b"new\n")
        .unwrap();
    assert_eq!(resolve(&profile(&path)).unwrap().0, "new");
    // Run with the ordinary test user's credentials: root bypasses permission bits.
    if os::effective_uid() != 0 {
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o311)).unwrap();
        let read = resolve(&profile(&path));
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(read.unwrap().0, "new");
    }
    assert_eq!(
        resolve(&profile(&root.join("missing")))
            .err()
            .unwrap()
            .code(),
        "authentication_required"
    );
}

#[test]
fn malformed_secret_errors_never_retain_the_input() {
    for text in ["canary\nprivate", "canary private", "canary\0private"] {
        let error = Secret(text.into()).validate().err().unwrap();
        assert!(!format!("{error:?}").contains("canary"));
        assert!(!serde_json::to_string(&error).unwrap().contains("canary"));
    }
}
