use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn absent_name_retry_and_empty_sweep_repeat_the_parent_durability_barrier() {
    let _fork = crate::os::fork_guard().unwrap();
    let root = private_root().unwrap();
    let image = fixture(root.path());
    let original = image.directory.clone();
    let count = Cell::new(0);
    let result = state::sync_tests::with_hook(
        move |path, step| {
            if path == original && step == Step::BeforeDirectorySync {
                count.set(count.get() + 1);
                if count.get() == 2 {
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
            }
            Ok(())
        },
        || image.cleanup(),
    );
    assert!(result.is_err());
    let retirement = abandon(image);
    assert!(!retirement.original.exists() && !retirement.aside.exists());
    let _cache = state::Lock::index(&root.path().join("cache.lock")).unwrap();
    for empty_sweep in [false, true] {
        let expected = if empty_sweep {
            root.path().to_owned()
        } else {
            retirement.original.clone()
        };
        let hits = Arc::new(AtomicUsize::new(0));
        let reached = hits.clone();
        let result = state::sync_tests::with_hook(
            move |path, step| {
                if path == expected && step == Step::BeforeDirectorySync {
                    reached.fetch_add(1, Ordering::SeqCst);
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
                Ok(())
            },
            || {
                if empty_sweep {
                    crate::launch::image::sweep_checked(root.path(), None)
                } else {
                    retirement.remove(None)
                }
            },
        );
        assert!(result.is_err());
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
    retirement.remove(None).unwrap();
    crate::launch::image::sweep_checked(root.path(), None).unwrap();
}
