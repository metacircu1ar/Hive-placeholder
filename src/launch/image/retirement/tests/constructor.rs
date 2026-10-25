use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn parent_creation_sync_failure_prevents_copy_even_after_owner_marker_is_durable() {
    let _fork = crate::os::fork_guard().unwrap();
    for fail_at in [Step::BeforeDirectorySync, Step::AfterDirectorySync] {
        let root = private_root().unwrap();
        fs::write(root.path().join("source"), b"launcher").unwrap();
        let expected = root.path().to_owned();
        let owner_synced = Cell::new(false);
        let hits = Arc::new(AtomicUsize::new(0));
        let observed = hits.clone();
        let result = state::sync_tests::with_hook(
            move |path, step| {
                if path.file_name().is_some_and(|n| n == "owner.lock")
                    && step == Step::AfterDirectorySync
                {
                    owner_synced.set(true);
                }
                if path == expected && owner_synced.get() && step == fail_at {
                    observed.fetch_add(1, Ordering::SeqCst);
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
                Ok(())
            },
            || {
                build_image(root.path(), &root.path().join("source"), |_| {
                    panic!("copy reached before parent sync")
                })
            },
        );
        assert!(result.is_err());
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        sweep(root.path());
        assert!(
            !fs::read_dir(root.path())
                .unwrap()
                .any(|e| e.unwrap().path().is_dir())
        );
    }
}

#[test]
fn constructor_failure_before_ownership_never_recursively_removes_unknown_contents() {
    let _fork = crate::os::fork_guard().unwrap();
    for occupied in [true, false] {
        let root = private_root().unwrap();
        let sentinel = root.path().join("sentinel");
        state::private_file(&sentinel, true)
            .unwrap()
            .write_all(b"untouched")
            .unwrap();
        let retained = sentinel.clone();
        // The source is hashed before any reservation is made.
        fs::write(root.path().join("source"), b"launcher").unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let observed = hits.clone();
        let result = state::sync_tests::with_checkpoint(
            move |path, step| {
                if step == hive_setup_support::checkpoints::Checkpoint::BeforeIntent {
                    observed.fetch_add(1, Ordering::SeqCst);
                    if occupied {
                        // Lock acquisition rejects the unowned multiply-linked file.
                        fs::hard_link(&retained, path.join("owner.lock"))?;
                    } else {
                        return Err(io::Error::from_raw_os_error(libc::EIO));
                    }
                }
                Ok(())
            },
            || {
                build_image(root.path(), &root.path().join("source"), |_| {
                    panic!("copy reached without ownership")
                })
            },
        );
        let Err(error) = result else {
            panic!("construction succeeded");
        };
        assert_eq!(
            error.code(),
            if occupied { "unsafe_state" } else { "state_io" }
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        let directories = fs::read_dir(root.path())
            .unwrap()
            .filter(|e| e.as_ref().unwrap().path().is_dir())
            .count();
        assert_eq!(directories, usize::from(occupied));
        sweep(root.path());
        assert_eq!(fs::read(sentinel).unwrap(), b"untouched");
    }
}
