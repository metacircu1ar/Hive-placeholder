use super::*;

fn image(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>, u64)> {
    let mut result = Vec::new();
    let mut pending = vec![root.to_owned()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(&path).unwrap() {
            let entry = entry.unwrap();
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                pending.push(entry.path());
            } else {
                result.push((entry.path(), fs::read(entry.path()).unwrap(), meta.ino()));
            }
        }
    }
    result.sort();
    result
}

#[test]
fn upgrade_preflight_retains_files_locks_and_does_not_run_pending_mutations() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("knowledge");
    let comb = crate::Comb::init(&root).unwrap();
    assert!(upgrade(&root, true).is_err());
    drop(comb);
    let before = image(temp.path());
    for dry_run in [true, false] {
        let report = upgrade(&root, dry_run).unwrap();
        assert_eq!(report.status, "current");
        assert!(report.steps.is_empty());
        assert!(report.backups.is_empty());
        assert_eq!(image(temp.path()), before);
    }
    hive_setup_support::files::write(
        &root.join(".comb/intent.json"),
        b"retained malformed intent",
        false,
    )
    .unwrap();
    let before = image(temp.path());
    assert!(upgrade(&root, true).is_err());
    assert_eq!(image(temp.path()), before);
}

#[test]
fn upgrade_never_initializes_missing_root_and_refuses_replaced_lock() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("knowledge");
    assert!(upgrade(&root, true).is_err());
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    drop(crate::Comb::init(&root).unwrap());
    let owner = Owner::acquire(&root).unwrap();
    assert!(crate::Comb::open(&root).is_err());
    fs::rename(root.join(".comb/lock"), root.join(".comb/retained.lock")).unwrap();
    assert!(owner.check().is_err());
    drop(owner);
    assert!(upgrade(&root, false).is_err());
    assert!(!root.join(".comb/lock").exists());
}

#[test]
fn preflight_detects_database_damage_without_changing_original_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("knowledge");
    drop(crate::Comb::init(&root).unwrap());
    let db = rusqlite::Connection::open(root.join(".comb/index.sqlite")).unwrap();
    db.execute_batch("PRAGMA ignore_check_constraints=ON; INSERT INTO embedding_space VALUES(2,'bad','bad','{}');").unwrap();
    drop(db);
    let before = image(temp.path());
    assert_eq!(
        upgrade(&root, true).err().unwrap().code,
        crate::ErrorCode::IndexError
    );
    assert_eq!(image(temp.path()), before);
}

#[test]
fn root_exclusion_is_held_inside_sqlite_close() {
    use std::{
        ffi::c_void,
        sync::atomic::{AtomicBool, Ordering},
    };
    struct Probe {
        root: std::path::PathBuf,
        observed: AtomicBool,
    }
    unsafe extern "C" fn close(
        _: u32,
        context: *mut c_void,
        _: *mut c_void,
        _: *mut c_void,
    ) -> i32 {
        // SAFETY: the boxed probe remains alive until after Comb and SQLite drop.
        let probe = unsafe { &*context.cast::<Probe>() };
        let blocked = locks::existing(&probe.root, "comb-root")
            .is_err_and(|e| e.kind() == std::io::ErrorKind::WouldBlock);
        probe.observed.store(blocked, Ordering::SeqCst);
        0
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("knowledge");
    let comb = crate::Comb::init(&root).unwrap();
    let mut probe = Box::new(Probe {
        root: root.clone(),
        observed: AtomicBool::new(false),
    });
    // SAFETY: callback cannot unwind, pointer is stable, and registration dies
    // with this connection before its boxed context is dropped.
    let result = unsafe {
        rusqlite::ffi::sqlite3_trace_v2(
            comb.index.test_connection().handle(),
            rusqlite::ffi::SQLITE_TRACE_CLOSE,
            Some(close),
            std::ptr::from_mut(probe.as_mut()).cast(),
        )
    };
    assert_eq!(result, rusqlite::ffi::SQLITE_OK);
    drop(comb);
    assert!(probe.observed.load(Ordering::SeqCst));
    drop(locks::existing(&root, "comb-root").unwrap());
}
