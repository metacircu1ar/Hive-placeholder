use super::*;
use crate::disk::publication_hooks as hooks;
use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub(super) fn chmod(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn socket_privacy_refuses_public_leaves_and_unsafe_ancestors_before_creating_a_lock() {
    let (root, _) = fixture();
    let parent = root.path().join("parent");
    fs::create_dir(&parent).unwrap();
    let parent = parent.canonicalize().unwrap();
    let nested = parent.join("private");
    fs::create_dir(&nested).unwrap();
    chmod(&nested, 0o700);
    for mode in [0o755, 0o750, 0o770, 0o775, 0o777, 0o1777] {
        chmod(&parent, mode);
        assert!(Lease::acquire(root.path(), parent.join("s.sock")).is_err());
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 1);
        let refused = if rustix::process::geteuid().is_root() {
            mode & 0o022 != 0 && mode & 0o1000 == 0
        } else {
            mode & 0o002 != 0
        };
        if refused {
            assert!(Lease::acquire(root.path(), nested.join("s.sock")).is_err());
            assert_eq!(fs::read_dir(&nested).unwrap().count(), 0);
        }
    }
    chmod(&parent, 0o700);
    drop(Lease::acquire(root.path(), nested.join("s.sock")).unwrap());
    // /tmp itself may be a root-owned sticky ancestor, but is never a private leaf.
    let sticky = Path::new("/tmp").canonicalize().unwrap();
    assert!(directory::check(&sticky, directory::Action::Existing).is_err());
}

#[test]
fn socket_privacy_rechecks_permissions_before_bind_reclaim_and_cleanup() {
    let (root, path) = fixture();
    let lease = Lease::acquire(root.path(), path.clone()).unwrap();
    chmod(root.path(), 0o755);
    assert!(lease.bind().is_err());
    assert!(!path.exists());
    chmod(root.path(), 0o700);
    abandoned(root.path(), &path);
    let mut lease = Lease::acquire(root.path(), path.clone()).unwrap();
    let comb = Comb::open(root.path()).unwrap();
    let before = fs::read(receipt_path(&path)).unwrap();
    chmod(root.path(), 0o755);
    assert!(lease.reclaim(&comb.disk).is_err());
    assert!(path.exists());
    assert_eq!(fs::read(receipt_path(&path)).unwrap(), before);
    chmod(root.path(), 0o700);
    lease.reclaim(&comb.disk).unwrap();
    let (listener, mut guard) = lease.bind().unwrap();
    drop(listener);
    let before = fs::read(receipt_path(&path)).unwrap();
    chmod(root.path(), 0o755);
    assert!(guard.cleanup().is_err());
    assert!(path.exists());
    assert_eq!(fs::read(receipt_path(&path)).unwrap(), before);
    chmod(root.path(), 0o700);
    guard.cleanup().unwrap();
}

#[test]
fn socket_privacy_refuses_permission_changes_at_the_actual_bind_boundary() {
    let (root, path) = fixture();
    let lease = Lease::acquire(root.path(), path.clone()).unwrap();
    let selected = path.clone();
    let parent = root.path().to_path_buf();
    let error = hooks::with(
        move |at, step| {
            if at == selected && step == atomic_fs::Step::Written {
                fs::set_permissions(&parent, fs::Permissions::from_mode(0o755))?;
            }
            Ok(())
        },
        || lease.bind().err().unwrap(),
    );
    assert!(crate::service::auth::has_cleanup_failure(&error));
    assert!(path.exists());
    assert!(!receipt_path(&path).exists());
    chmod(root.path(), 0o700);
    assert!(Lease::acquire(root.path(), path).is_err());
}

struct Owned(Child);
impl Drop for Owned {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
            self.0.wait().unwrap();
        }
    }
}
#[test]
fn socket_privacy_is_established_before_chmod_under_permissive_umask() {
    let mut child = Owned(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "service::socket::tests::privacy::umask_fixture",
                "--ignored",
            ])
            .env("COMB_SOCKET_UMASK_FIXTURE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success());
            return;
        }
        assert!(Instant::now() < deadline, "owned socket fixture timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
#[ignore = "owned permissive-umask fixture invoked by the socket privacy test"]
fn umask_fixture() {
    assert_eq!(std::env::var("COMB_SOCKET_UMASK_FIXTURE").unwrap(), "1");
    // SAFETY: this owned child runs only this exact test; no other test can create files.
    unsafe {
        libc::umask(0);
    }
    let (root, path) = fixture();
    let nested = root.path().canonicalize().unwrap().join("new/deep");
    directory::check(&nested, directory::Action::Create).unwrap();
    for directory in [&nested, nested.parent().unwrap()] {
        assert_eq!(
            fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    let selected = path.clone();
    let observed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = observed.clone();
    let (listener, mut guard) = hooks::with(
        move |at, step| {
            if at == selected && step == atomic_fs::Step::Written {
                assert_eq!(
                    fs::metadata(at.parent().unwrap())?.permissions().mode() & 0o777,
                    0o700
                );
                assert_eq!(
                    fs::symlink_metadata(at)?.permissions().mode() & 0o777,
                    0o777
                );
                assert_eq!(
                    comb_protocol::service::check_socket(at).unwrap_err().code,
                    crate::ErrorCode::UnsafeSocket
                );
                assert!(
                    super::readiness::probe(at.parent().unwrap(), at)
                        .unwrap()
                        .is_none()
                );
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            Ok(())
        },
        || {
            Lease::acquire(root.path(), path.clone())
                .unwrap()
                .bind()
                .unwrap()
        },
    );
    assert!(observed.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(
        fs::symlink_metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    comb_protocol::service::check_socket(&path).unwrap();
    drop(listener);
    guard.cleanup().unwrap();
}
