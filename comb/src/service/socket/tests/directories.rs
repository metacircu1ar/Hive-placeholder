use super::{privacy::chmod, *};
use crate::disk::publication_hooks as hooks;

#[test]
fn search_only_ancestors_allow_existing_sockets_and_durable_creation_below_readable_parents() {
    let (root, _) = fixture();
    let ancestor = root.path().canonicalize().unwrap().join("search-only");
    let parent = ancestor.join("readable");
    let leaf = parent.join("private");
    fs::create_dir_all(&leaf).unwrap();
    chmod(&parent, 0o700);
    chmod(&leaf, 0o700);
    chmod(&ancestor, 0o311);
    let result = (|| -> Result<()> {
        let (listener, mut guard) = Lease::acquire(root.path(), leaf.join("s.sock"))?.bind()?;
        drop(listener);
        guard.cleanup()?;
        let child = leaf.join("new/deep");
        directory::check(&child, directory::Action::Create)?;
        directory::check(&child, directory::Action::Create)?;
        Ok(())
    })();
    // A required ancestor barrier needs a readable descriptor, even on retry.
    let blocked = parent.join("new");
    let first = directory::check(&blocked, directory::Action::Create);
    let retry = directory::check(&blocked, directory::Action::Create);
    let direct = directory::check(&ancestor.join("new"), directory::Action::Create);
    chmod(&ancestor, 0o700);
    result.unwrap();
    if !rustix::process::geteuid().is_root() {
        assert!(first.is_err());
        assert!(retry.is_err());
        assert!(direct.is_err());
        assert!(!blocked.exists());
        assert!(!ancestor.join("new").exists());
    }
}

#[test]
fn required_directory_barriers_keep_erofs_failures_on_retry() {
    for fail_parent in [false, true] {
        let (root, _) = fixture();
        let parent = root.path().canonicalize().unwrap();
        let created = parent.join("new");
        let target = created.join("deep");
        let watched = created.clone();
        let failing = if fail_parent { parent } else { created.clone() };
        hooks::with(
            move |path, at| {
                if path == failing && at == atomic_fs::Step::BeforeDirectorySync && watched.exists()
                {
                    return Err(std::io::Error::from_raw_os_error(libc::EROFS));
                }
                Ok(())
            },
            || {
                for _ in 0..2 {
                    let error = directory::check(&target, directory::Action::Create).unwrap_err();
                    assert_eq!(error.code, crate::ErrorCode::IoError);
                    assert!(error.diagnostic().unwrap().contains("Read-only"));
                    assert!(created.exists());
                    assert!(!target.exists());
                }
            },
        );
        directory::check(&target, directory::Action::Create).unwrap();
        directory::check(&target, directory::Action::Create).unwrap();
    }
}

#[test]
fn group_writable_ancestors_allow_private_socket_lifecycle_and_directory_retry() {
    let (root, _) = fixture();
    let ancestor = root.path().canonicalize().unwrap().join("group-layout");
    let leaf = ancestor.join("private");
    fs::create_dir_all(&leaf).unwrap();
    chmod(&ancestor, 0o775);
    chmod(&leaf, 0o700);
    let socket = leaf.join("s.sock");
    if rustix::process::geteuid().is_root() {
        assert!(Lease::acquire(root.path(), socket).is_err());
        return;
    }
    chmod(root.path(), 0o775);
    let (listener, mut guard) = Lease::acquire(root.path(), socket.clone())
        .unwrap()
        .bind()
        .unwrap();
    comb_protocol::service::check_socket(&socket).unwrap();
    let client = std::os::unix::net::UnixStream::connect(&socket).unwrap();
    drop(client);
    drop(listener);
    guard.cleanup().unwrap();
    assert!(!socket.exists());
    let created = ancestor.join("new/deep");
    for _ in 0..2 {
        directory::check(&created, directory::Action::Create).unwrap();
        assert_eq!(
            fs::metadata(&ancestor).unwrap().permissions().mode() & 0o777,
            0o775
        );
        for private in [&leaf, &created, created.parent().unwrap()] {
            assert_eq!(
                fs::metadata(private).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }
}
