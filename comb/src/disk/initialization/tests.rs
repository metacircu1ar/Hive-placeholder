use super::*;
use crate::disk::{Disk, durable::hooks};
use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    time::Duration,
};

const BOUNDARIES: [(&str, Step); 19] = [
    (".gitignore", Step::Written),
    (".gitignore", Step::AfterPublish),
    (".gitignore", Step::AfterDirectorySync),
    ("heads", Step::AfterPublish),
    ("receipts", Step::AfterPublish),
    ("lock", Step::Written),
    ("lock", Step::AfterPublish),
    ("identity.json", Step::Written),
    ("identity.json", Step::AfterPublish),
    ("identity.json", Step::AfterDirectorySync),
    ("format", Step::Written),
    ("format", Step::AfterPublish),
    ("format", Step::AfterDirectorySync),
    (".comb", Step::BeforeFileSync),
    (".comb", Step::AfterFileSync),
    (".comb", Step::BeforePublish),
    (".comb", Step::AfterPublish),
    (".comb", Step::BeforeDirectorySync),
    (".comb", Step::AfterDirectorySync),
];
const PUBLISHED: usize = BOUNDARIES.len() - 3;
fn selected(path: &Path, step: Step, index: usize) -> bool {
    path.file_name()
        .is_some_and(|name| name == BOUNDARIES[index].0)
        && step == BOUNDARIES[index].1
}
fn preparation(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".comb-tmp-")
        })
        .collect()
}
fn protection(path: &Path) {
    assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o700);
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        let meta = fs::symlink_metadata(&path).unwrap();
        assert_eq!(meta.mode() & 0o077, 0);
        if meta.is_file() && fs::read(&path).unwrap().windows(5).any(|s| s == b"\"key\"") {
            assert_eq!(
                fs::read(path.parent().unwrap().join(".gitignore")).unwrap(),
                b"*\n"
            );
        }
    }
}
fn complete(root: &Path) -> Vec<u8> {
    let admin = root.join(".comb");
    protection(&admin);
    for name in ["heads", "receipts"] {
        assert!(admin.join(name).is_dir());
    }
    assert!(fs::read(admin.join("lock")).unwrap().is_empty());
    assert_eq!(fs::read(admin.join(".gitignore")).unwrap(), b"*\n");
    let bytes = fs::read(admin.join("identity.json")).unwrap();
    let _: Identity = serde_json::from_slice(&bytes).unwrap();
    let marker: hive_input::compatibility::Marker =
        serde_json::from_slice(&fs::read(admin.join("format")).unwrap()).unwrap();
    assert_eq!(
        marker,
        hive_input::compatibility::Marker::current(&crate::compatibility::ROOT)
    );
    bytes
}

#[test]
fn marker_refusal_preserves_existing_root_identity_and_content() {
    for marker in [
        None,
        Some(b"{\"format\":2}".as_slice()),
        Some(b"[1]".as_slice()),
    ] {
        let temp = tempfile::tempdir().unwrap();
        drop(Disk::acquire(temp.path(), true).unwrap());
        let admin = temp.path().join(".comb");
        let identity = fs::read(admin.join("identity.json")).unwrap();
        if let Some(bytes) = marker {
            fs::write(admin.join("format"), bytes).unwrap();
        } else {
            fs::remove_file(admin.join("format")).unwrap();
        }
        for initialize in [false, true] {
            assert!(Disk::acquire(temp.path(), initialize).is_err());
            assert!(Disk::human_authority(temp.path()).is_err());
            assert_eq!(fs::read(admin.join("identity.json")).unwrap(), identity);
            if let Some(bytes) = marker {
                assert_eq!(fs::read(admin.join("format")).unwrap(), bytes);
            } else {
                assert!(!admin.join("format").exists());
            }
        }
    }
}

#[test]
fn every_initialization_fault_leaves_absent_or_complete_admin_and_retry_keeps_identity() {
    for index in 0..BOUNDARIES.len() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let error = hooks::with(
            move |path, step| {
                if selected(path, step, index) {
                    Err(io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
            || Disk::acquire(root, true),
        )
        .err()
        .unwrap();
        assert_eq!(error.file_committed, index >= PUBLISHED);
        assert_eq!(error.code, crate::ErrorCode::IoError);
        let before = if index >= PUBLISHED {
            Some(complete(root))
        } else {
            assert!(!root.join(".comb").exists());
            None
        };
        let stages = preparation(root);
        assert_eq!(stages.len(), usize::from(index < PUBLISHED));
        for stage in &stages {
            protection(stage);
        }
        drop(Disk::acquire(root, true).unwrap());
        if let Some(before) = before {
            assert_eq!(complete(root), before);
        }
        for stage in &stages {
            protection(stage);
        }
    }
}

#[test]
fn publication_parent_sync_repeats_until_success_and_never_replaces_identity() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let admin = root.join(".comb");
    let expected = admin.clone();
    let error = hooks::with(
        move |path, step| {
            if path == expected && step == Step::BeforeDirectorySync {
                Err(io::Error::from_raw_os_error(libc::EIO))
            } else {
                Ok(())
            }
        },
        || Disk::acquire(&root, true),
    )
    .err()
    .unwrap();
    assert!(error.file_committed);
    let before = complete(&root);
    let recording = Arc::new(crate::disk::Recording::default());
    recording.fail(crate::disk::Operation::DirectorySync, 1);
    let error = Disk::acquire_with(&root, false, recording.clone())
        .err()
        .unwrap();
    assert!(error.file_committed);
    assert_eq!(
        recording.counts().events,
        [crate::disk::Operation::DirectorySync]
    );
    assert_eq!(complete(&root), before);
    recording.reset();
    drop(Disk::acquire_with(&root, false, recording.clone()).unwrap());
    assert_eq!(
        recording.counts().events,
        [crate::disk::Operation::DirectorySync]
    );
    assert_eq!(complete(&root), before);
}

#[test]
fn identity_preparation_unwind_keeps_ignore_protection_and_retry_ignores_stage() {
    let temp = tempfile::tempdir().unwrap();
    assert!(
        std::panic::catch_unwind(|| hooks::with(
            |path, step| {
                assert!(
                    !(path.file_name().unwrap() == "identity.json" && step == Step::Written),
                    "owned identity fixture"
                );
                Ok(())
            },
            || Disk::acquire(temp.path(), true)
        ))
        .is_err()
    );
    let stages = preparation(temp.path());
    assert_eq!(stages.len(), 1);
    protection(&stages[0]);
    assert_eq!(fs::read(stages[0].join(".gitignore")).unwrap(), b"*\n");
    drop(crate::Comb::init(temp.path()).unwrap());
    assert!(stages[0].exists());
    complete(temp.path());
}

#[test]
fn active_initialization_excludes_peers_and_publishes_only_complete_admin() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (arrived, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let writer_root = root.clone();
    let writer = std::thread::spawn(move || {
        hooks::with(
            move |path, step| {
                if path.file_name().unwrap() == ".comb"
                    && matches!(step, Step::BeforePublish | Step::AfterPublish)
                {
                    arrived.send(step).unwrap();
                    released.recv_timeout(Duration::from_secs(10)).unwrap();
                }
                Ok(())
            },
            || {
                drop(Disk::acquire(&writer_root, true).unwrap());
            },
        );
    });
    assert_eq!(
        waiting.recv_timeout(Duration::from_secs(10)).unwrap(),
        Step::BeforePublish
    );
    assert!(!root.join(".comb").exists());
    assert_eq!(
        Disk::acquire(&root, true).err().unwrap().code,
        crate::ErrorCode::CombBusy
    );
    release.send(()).unwrap();
    assert_eq!(
        waiting.recv_timeout(Duration::from_secs(10)).unwrap(),
        Step::AfterPublish
    );
    complete(&root);
    assert_eq!(
        Disk::acquire(&root, false).err().unwrap().code,
        crate::ErrorCode::CombBusy
    );
    release.send(()).unwrap();
    writer.join().unwrap();
    drop(Disk::acquire(&root, false).unwrap());
}

#[test]
fn existing_pages_and_identity_survive_and_partial_admin_is_never_repaired() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("page.md"), b"authored content").unwrap();
    drop(Disk::acquire(temp.path(), true).unwrap());
    let before = complete(temp.path());
    drop(Disk::acquire(temp.path(), true).unwrap());
    assert_eq!(complete(temp.path()), before);
    assert_eq!(
        fs::read(temp.path().join("page.md")).unwrap(),
        b"authored content"
    );
    fs::remove_file(temp.path().join(".comb/lock")).unwrap();
    assert!(Disk::acquire(temp.path(), true).is_err());
    assert!(!temp.path().join(".comb/lock").exists());
    assert_eq!(
        fs::read(temp.path().join(".comb/identity.json")).unwrap(),
        before
    );
    let empty = tempfile::tempdir().unwrap();
    assert!(Disk::acquire(empty.path(), false).is_err());
    assert!(!empty.path().join(".comb").exists());
}

mod directories;
mod process;

#[test]
fn fresh_postpublication_open_failure_reports_committed_and_preserves_identity() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let expected = root.join(".comb");
    let error = hooks::with(
        move |path, step| {
            if path == expected && step == Step::AfterDirectorySync {
                fs::rename(path.join("lock"), path.join("saved-lock"))?;
            }
            Ok(())
        },
        || Disk::acquire(&root, true),
    )
    .err()
    .unwrap();
    assert!(error.file_committed);
    assert_eq!(error.code, crate::ErrorCode::IoError);
    let before = fs::read(root.join(".comb/identity.json")).unwrap();
    // Existing-state admission still refuses corruption without claiming a write.
    assert!(!Disk::acquire(&root, false).err().unwrap().file_committed);
    fs::rename(root.join(".comb/saved-lock"), root.join(".comb/lock")).unwrap();
    drop(Disk::acquire(&root, false).unwrap());
    assert_eq!(complete(&root), before);
}

#[test]
fn normal_acquisition_retries_migration_completion_before_opening_sqlite() {
    let temp = tempfile::tempdir().unwrap();
    drop(Disk::acquire(temp.path(), true).unwrap());
    let admin = temp.path().join(".comb");
    hive_setup_support::files::directory(&admin.join("migrations")).unwrap();
    let identity = fs::read(admin.join("identity.json")).unwrap();
    for _ in 0..2 {
        let result = hooks::with(
            |path, step| {
                if path.file_name().is_some_and(|name| name == ".comb")
                    && step == Step::BeforeDirectorySync
                {
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
                Ok(())
            },
            || Disk::acquire(temp.path(), false),
        );
        assert!(result.is_err());
        assert!(!admin.join("index.sqlite").exists());
        assert_eq!(fs::read(admin.join("identity.json")).unwrap(), identity);
    }
    drop(Disk::acquire(temp.path(), false).unwrap());
}
