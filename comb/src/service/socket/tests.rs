use super::*;
use crate::{
    Comb,
    service::{Options, Server},
};
use std::{fs, os::unix::fs::symlink};
mod directories;
mod privacy;
mod readiness;

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    drop(Comb::init(dir.path()).unwrap());
    let path = super::super::socket_path(dir.path(), Some(&dir.path().join("s.sock"))).unwrap();
    (dir, path)
}

#[test]
fn unsupported_root_is_refused_before_socket_lock_creation() {
    for bytes in [None, Some(b"{\"format\":2}".as_slice())] {
        let (dir, path) = fixture();
        let marker = dir.path().join(".comb/format");
        if let Some(bytes) = bytes {
            fs::write(&marker, bytes).unwrap();
        } else {
            fs::remove_file(&marker).unwrap();
        }
        let names = || {
            let mut entries: Vec<_> = fs::read_dir(dir.path())
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            entries.sort();
            entries
        };
        let before = names();
        assert!(Lease::acquire(dir.path(), path).is_err());
        assert_eq!(names(), before);
    }
}
fn abandoned(root: &Path, path: &Path) -> Receipt {
    let (listener, mut guard) = Lease::acquire(root, path.into()).unwrap().bind().unwrap();
    let receipt = guard.receipt.clone();
    guard.pending = false; // Simulate abrupt exit after publication, with no cleanup.
    drop(listener);
    drop(guard);
    receipt
}
fn receipt_path(path: &Path) -> PathBuf {
    let hash = format!("{:x}", Sha256::digest(path.as_os_str().as_encoded_bytes()));
    path.with_file_name(format!(".comb-socket-{hash}.json"))
}
#[test]
fn exclusive_lease_and_receipt_keep_roots_separate() {
    let (dir, path) = fixture();
    let other = dir.path().join("other");
    drop(Comb::init(&other).unwrap());
    let lease = Lease::acquire(dir.path(), path.clone()).unwrap();
    assert!(Lease::acquire(dir.path(), path.clone()).is_err());
    assert!(Lease::acquire(&other, path.clone()).is_err());
    drop(lease);
    abandoned(dir.path(), &path);
    assert!(Lease::acquire(&other, path.clone()).is_err());
    let server = Server::open(
        dir.path(),
        Options {
            socket: Some(path.clone()),
            ..Options::default()
        },
    )
    .unwrap();
    drop(server);
    assert!(!path.exists());
    assert!(!receipt_path(&path).exists());
    drop(Lease::acquire(&other, path).unwrap());
}
#[test]
fn missing_malformed_arrays_and_foreign_receipts_never_remove_a_socket() {
    let (dir, path) = fixture();
    let receipt = abandoned(dir.path(), &path);
    let record = receipt_path(&path);
    let original = fs::read(&record).unwrap();
    for bytes in [
        b"[]".to_vec(),
        b"{".to_vec(),
        {
            let mut value = serde_json::to_value(&receipt).unwrap();
            value["entry"]["node"] = serde_json::json!([1, 2]);
            serde_json::to_vec(&value).unwrap()
        },
        {
            let mut value = serde_json::to_value(&receipt).unwrap();
            value["path"] = serde_json::json!("/other.sock");
            serde_json::to_vec(&value).unwrap()
        },
    ] {
        fs::write(&record, &bytes).unwrap();
        assert!(Lease::acquire(dir.path(), path.clone()).is_err());
        assert!(path.exists());
        assert_eq!(fs::read(&record).unwrap(), bytes);
    }
    fs::remove_file(&record).unwrap();
    assert!(Lease::acquire(dir.path(), path.clone()).is_err());
    let target = dir.path().join("foreign.json");
    fs::write(&target, &original).unwrap();
    symlink(&target, &record).unwrap();
    assert!(Lease::acquire(dir.path(), path.clone()).is_err());
    assert!(path.exists());
    assert_eq!(fs::read(target).unwrap(), original);
}
#[test]
fn replaced_socket_and_parent_survive_cleanup() {
    let (dir, path) = fixture();
    let (listener, mut guard) = Lease::acquire(dir.path(), path.clone())
        .unwrap()
        .bind()
        .unwrap();
    let old = path.with_extension("old");
    fs::rename(&path, &old).unwrap();
    let replacement = std::os::unix::net::UnixListener::bind(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    drop(listener);
    assert!(guard.cleanup().is_err());
    assert!(path.exists());
    drop(replacement);
    drop(guard);
    assert!(Lease::acquire(dir.path(), path.clone()).is_err());
    let lease = {
        fs::remove_file(&path).unwrap();
        fs::remove_file(receipt_path(&path)).unwrap();
        Lease::acquire(dir.path(), path.clone()).unwrap()
    };
    let moved = dir.path().with_extension("moved");
    fs::rename(dir.path(), &moved).unwrap();
    fs::create_dir(dir.path()).unwrap();
    assert!(lease.check().is_err());
    drop(lease);
    fs::remove_dir(dir.path()).unwrap();
    fs::rename(moved, dir.path()).unwrap();
}
struct FailPublish;
impl atomic_fs::Barriers for FailPublish {
    fn observe(&self, step: atomic_fs::Step) -> std::io::Result<()> {
        if step == atomic_fs::Step::BeforePublish {
            Err(std::io::Error::other("injected publish failure"))
        } else {
            Ok(())
        }
    }
}
#[test]
fn publication_failure_cleans_owned_socket_and_releases_lease() {
    let (dir, path) = fixture();
    let error = Lease::acquire(dir.path(), path.clone())
        .unwrap()
        .bind_with(&FailPublish)
        .err()
        .unwrap();
    assert!(!error.file_committed);
    assert!(!path.exists());
    assert!(!receipt_path(&path).exists());
    assert!(!fs::read_dir(dir.path()).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".comb-tmp-")
    }));
    drop(Lease::acquire(dir.path(), path).unwrap());
}
#[test]
fn bind_without_receipt_is_unproven_and_absent_owned_socket_is_recoverable() {
    let (dir, path) = fixture();
    let lease = Lease::acquire(dir.path(), path.clone()).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    drop(listener);
    drop(lease);
    assert!(Lease::acquire(dir.path(), path.clone()).is_err());
    assert!(path.exists());
    fs::remove_file(&path).unwrap();
    abandoned(dir.path(), &path);
    fs::remove_file(&path).unwrap();
    let mut lease = Lease::acquire(dir.path(), path.clone()).unwrap();
    let comb = Comb::open_checked(dir.path(), &|disk| lease.check_disk(disk)).unwrap();
    lease.reclaim(&comb.disk).unwrap();
    assert!(!receipt_path(&path).exists());
}

struct FailCleanup(PathBuf);
impl atomic_fs::Barriers for FailCleanup {
    fn observe(&self, step: atomic_fs::Step) -> std::io::Result<()> {
        if step == atomic_fs::Step::BeforePublish {
            fs::set_permissions(&self.0, fs::Permissions::from_mode(0o777))?;
            Err(std::io::Error::other(
                "injected publication and cleanup failure",
            ))
        } else {
            Ok(())
        }
    }
}
#[test]
fn cleanup_failure_preserves_the_primary_error_and_uncertain_entry() {
    let (dir, path) = fixture();
    let error = Lease::acquire(dir.path(), path.clone())
        .unwrap()
        .bind_with(&FailCleanup(dir.path().into()))
        .err()
        .unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(crate::service::auth::has_cleanup_failure(&error));
    assert!(error.diagnostic().unwrap().contains("injected publication"));
    assert!(path.exists());
    assert!(Lease::acquire(dir.path(), path).is_err());
}

#[test]
fn replaced_root_is_checked_before_index_population() {
    let (dir, path) = fixture();
    let root = dir.path().join("root");
    drop(Comb::init(&root).unwrap());
    let lease = Lease::acquire(&root, path).unwrap();
    fs::rename(&root, dir.path().join("old")).unwrap();
    drop(Comb::init(&root).unwrap());
    fs::remove_file(root.join(".comb/index.sqlite")).unwrap();
    assert!(Comb::open_checked(&root, &|disk| lease.check_disk(disk)).is_err());
    assert!(!root.join(".comb/index.sqlite").exists());
}

#[test]
fn lock_symlinks_hardlinks_and_replaced_entries_are_refused() {
    let (dir, path) = fixture();
    let lease = Lease::acquire(dir.path(), path.clone()).unwrap();
    let lock = path.with_file_name(&lease.lock_name);
    fs::rename(&lock, lock.with_extension("old")).unwrap();
    fs::write(&lock, b"").unwrap();
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(lease.check().is_err());
    drop(lease);
    fs::remove_file(&lock).unwrap();
    symlink(lock.with_extension("old"), &lock).unwrap();
    assert!(Lease::acquire(dir.path(), path.clone()).is_err());
    fs::remove_file(&lock).unwrap();
    fs::hard_link(lock.with_extension("old"), &lock).unwrap();
    assert!(Lease::acquire(dir.path(), path).is_err());
}

#[test]
fn duplicate_receipt_fields_are_rejected_without_removing_evidence() {
    let (dir, path) = fixture();
    let receipt = abandoned(dir.path(), &path);
    let record = receipt_path(&path);
    let original = serde_json::to_string(&receipt).unwrap();
    for bytes in [
        original.replacen(
            '{',
            &format!(
                "{{\"path\":{},",
                serde_json::to_string(&receipt.path).unwrap()
            ),
            1,
        ),
        original.replacen("\"entry\":{", "\"entry\":{\"ctime\":0,", 1),
        original.replacen("\"node\":{", "\"node\":{\"inode\":0,", 1),
    ] {
        fs::write(&record, &bytes).unwrap();
        assert!(Lease::acquire(dir.path(), path.clone()).is_err());
        assert!(path.exists());
        assert_eq!(fs::read_to_string(&record).unwrap(), bytes);
    }
}

#[test]
fn native_permission_setup_never_chmods_a_replacement_symlink() {
    use std::os::unix::fs::MetadataExt;
    let (dir, path) = fixture();
    let lease = Lease::acquire(dir.path(), path.clone()).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let node = Node::of(&lease.parent.stat(lease.name().unwrap()).unwrap().unwrap());
    lease
        .parent
        .private_socket(lease.name().unwrap(), &node)
        .unwrap();
    assert_eq!(fs::symlink_metadata(&path).unwrap().mode() & 0o777, 0o600);
    fs::rename(&path, path.with_extension("old")).unwrap();
    let target = dir.path().join("foreign");
    fs::write(&target, b"preserve").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
    symlink(&target, &path).unwrap();
    assert!(
        lease
            .parent
            .private_socket(lease.name().unwrap(), &node)
            .is_err()
    );
    assert_eq!(fs::metadata(target).unwrap().mode() & 0o777, 0o644);
    drop(listener);
}

#[test]
fn reserved_socket_name_fails_before_lock_or_receipt_creation() {
    let temp = tempfile::tempdir().unwrap();
    for name in [".hive-tmp-state-ABC123", ".comb-tmp-admin-ABC123"] {
        let socket = temp.path().join(name);
        assert!(Lease::acquire(&temp.path().join("missing-root"), socket).is_err());
    }
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}
