use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Default)]
struct Barriers {
    attempts: AtomicUsize,
    fail: AtomicBool,
}
impl atomic_fs::Barriers for Barriers {
    fn observe(&self, step: atomic_fs::Step) -> Result<()> {
        if step == atomic_fs::Step::BeforeDirectorySync {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                return Err(std::io::Error::from_raw_os_error(libc::EIO));
            }
        }
        Ok(())
    }
}

#[test]
fn only_success_is_reused_and_new_owner_or_root_requires_a_new_barrier() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    files::directory(&root).unwrap();
    files::directory(&root.join("migrations")).unwrap();
    let barriers = Barriers::default();
    barriers.fail.store(true, Ordering::SeqCst);
    let first = Shared::acquire(&root, "test").unwrap();
    for _ in 0..2 {
        assert!(first.admit_writes(&root, &barriers).is_err());
    }
    assert_eq!(barriers.attempts.load(Ordering::SeqCst), 2);
    barriers.fail.store(false, Ordering::SeqCst);
    first.admit_writes(&root, &barriers).unwrap();
    let nested = Shared::acquire(&root, "test").unwrap();
    for _ in 0..10 {
        nested.admit_writes(&root, &barriers).unwrap();
    }
    assert_eq!(barriers.attempts.load(Ordering::SeqCst), 3);
    assert!(Exclusive::existing(&root, "test").is_err());
    files::write(&root.join(crate::migration::PENDING), b"{}", true).unwrap();
    assert!(nested.admit_writes(&root, &barriers).is_err());
    std::fs::remove_file(root.join(crate::migration::PENDING)).unwrap();
    std::fs::rename(&root, temp.path().join("old-root")).unwrap();
    files::directory(&root).unwrap();
    files::directory(&root.join("migrations")).unwrap();
    nested.admit_writes(&root, &barriers).unwrap();
    assert_eq!(barriers.attempts.load(Ordering::SeqCst), 4);
    drop(first);
    drop(nested);
    let fresh = Shared::acquire(&root, "test").unwrap();
    fresh.admit_writes(&root, &barriers).unwrap();
    assert_eq!(barriers.attempts.load(Ordering::SeqCst), 5);
}

#[test]
fn concurrent_nested_owners_settle_once_and_replaced_lock_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    files::directory(&root).unwrap();
    files::directory(&root.join("migrations")).unwrap();
    let first = Shared::acquire(&root, "test").unwrap();
    let barriers = Barriers::default();
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| first.admit_writes(&root, &barriers).unwrap());
        }
    });
    assert_eq!(barriers.attempts.load(Ordering::SeqCst), 1);
    std::fs::rename(&first.0.path, first.0.path.with_extension("old")).unwrap();
    assert!(first.admit_writes(&root, &barriers).is_err());
}
