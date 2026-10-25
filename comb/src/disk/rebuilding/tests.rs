use super::*;
use crate::{Actor, Comb, disk::publication_hooks as hooks, store::Write};
use atomic_fs::Step;
use std::{fs, io, path::Path};

mod automatic;
mod process;

fn fixture(reset_vectors: bool) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let mut comb = Comb::init(root.path()).unwrap();
    comb.put_markdown(
        &"concepts/kept".parse().unwrap(),
        "---\ntitle: Kept\n---\nReviewed knowledge",
        None,
        &Write::new(&Actor::Human, &"create".parse().unwrap(), None).unwrap(),
    )
    .unwrap();
    drop(comb);
    if reset_vectors {
        let admin = root.path().join(".comb");
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .write(admin.join("vectors.sqlite"), b"")
            .unwrap();
        for name in NAMES {
            if name.ends_with(".sqlite") {
                continue;
            }
            atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
                .create_new(admin.join(name), b"owned sidecar")
                .unwrap();
        }
    }
    root
}

fn recover(root: &Path) {
    let comb = Comb::open(root).unwrap();
    assert_eq!(comb.stats().unwrap().pages, 1);
    let page = comb.get(&"concepts/kept".parse().unwrap()).unwrap();
    assert!(!page.unverified);
    assert_eq!(page.page.compiled, "Reviewed knowledge");
    drop(comb);
    for name in [INTENT, POPULATION] {
        assert!(!root.join(".comb").join(name).exists());
    }
}

fn interrupt(root: &Path, name: &'static str, at: Step, occurrence: usize) -> Error {
    let count = std::cell::Cell::new(0);
    hooks::with(
        move |path, step| {
            if path.file_name().unwrap() == name && step == at {
                count.set(count.get() + 1);
                if count.get() == occurrence {
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
            }
            Ok(())
        },
        || Comb::rebuild(root),
    )
    .err()
    .unwrap()
}

const PUBLICATION: [Step; 7] = [
    Step::Written,
    Step::BeforeFileSync,
    Step::AfterFileSync,
    Step::BeforePublish,
    Step::AfterPublish,
    Step::BeforeDirectorySync,
    Step::AfterDirectorySync,
];
const DELETION: [Step; 4] = [
    Step::BeforeRemove,
    Step::AfterRemove,
    Step::BeforeDirectorySync,
    Step::AfterDirectorySync,
];

#[test]
fn every_selected_file_unlink_and_sync_failure_resumes_without_losing_knowledge() {
    for name in NAMES {
        for at in DELETION {
            let root = fixture(true);
            let error = interrupt(root.path(), name, at, 1);
            assert!(error.file_committed);
            assert!(root.path().join(".comb").join(INTENT).exists());
            recover(root.path());
        }
    }
}

#[test]
fn both_marker_publications_and_handoff_failures_keep_a_recoverable_generation() {
    for name in [INTENT, POPULATION] {
        for (position, step) in PUBLICATION.into_iter().enumerate() {
            let root = fixture(true);
            let error = interrupt(root.path(), name, step, 1);
            assert_eq!(error.file_committed, name == POPULATION || position >= 4);
            if name == INTENT && position < 4 {
                assert!(root.path().join(".comb/index.sqlite").exists());
                drop(Comb::rebuild(root.path()).unwrap());
            }
            recover(root.path());
        }
    }
    for step in DELETION {
        let root = fixture(true);
        // INTENT has an earlier directory sync during publication and re-admission.
        let occurrence = if matches!(step, Step::BeforeDirectorySync | Step::AfterDirectorySync) {
            3
        } else {
            1
        };
        assert!(interrupt(root.path(), INTENT, step, occurrence).file_committed);
        recover(root.path());
    }
    for step in [Step::BeforeFileSync, Step::AfterFileSync] {
        let root = fixture(true);
        // Population publication, repeat admission, then final handoff flush.
        assert!(interrupt(root.path(), POPULATION, step, 3).file_committed);
        assert!(!root.path().join(".comb").join(INTENT).exists());
        recover(root.path());
    }
}

#[test]
fn malformed_markers_refuse_before_deletion_and_offline_embedding_does_not_recover() {
    let root = fixture(false);
    let admin = root.path().join(".comb");
    let original = fs::read(admin.join("index.sqlite")).unwrap();
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .create_new(admin.join(POPULATION), b"invalid")
        .unwrap();
    assert!(Comb::rebuild(root.path()).is_err());
    assert_eq!(fs::read(admin.join("index.sqlite")).unwrap(), original);
    assert!(!admin.join(INTENT).exists());
    fs::remove_file(admin.join(POPULATION)).unwrap();
    interrupt(root.path(), INTENT, Step::AfterPublish, 1);
    let record = fs::read(admin.join(INTENT)).unwrap();
    let disk = Disk::acquire(root.path(), false).unwrap();
    assert_eq!(
        disk.reject_pending_rebuild().unwrap_err().code,
        crate::ErrorCode::RecoveryRequired
    );
    drop(disk);
    assert_eq!(fs::read(admin.join(INTENT)).unwrap(), record);
    assert_eq!(fs::read(admin.join("index.sqlite")).unwrap(), original);
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(admin.join(INTENT), b"{}")
        .unwrap();
    assert!(Comb::open(root.path()).is_err());
    assert_eq!(fs::read(admin.join("index.sqlite")).unwrap(), original);
}

#[test]
fn newly_appearing_or_replaced_files_refuse_before_any_destructive_retry() {
    for name in NAMES {
        let root = fixture(false);
        let admin = root.path().join(".comb");
        interrupt(root.path(), INTENT, Step::AfterPublish, 1);
        let main = fs::read(admin.join("index.sqlite")).unwrap();
        let original = admin.join(name);
        let saved = admin.join("saved-original");
        if original.exists() {
            fs::rename(&original, &saved).unwrap();
        }
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .create_new(&original, b"replacement")
            .unwrap();
        assert_eq!(
            Comb::open(root.path()).err().unwrap().code,
            crate::ErrorCode::UnsafePath
        );
        assert_eq!(fs::read(&original).unwrap(), b"replacement");
        if name != "index.sqlite" {
            assert_eq!(fs::read(admin.join("index.sqlite")).unwrap(), main);
        }
    }
}

#[test]
fn unsafe_candidates_refuse_before_acceptance() {
    for name in NAMES {
        for hardlink in [false, true] {
            let root = fixture(false);
            let admin = root.path().join(".comb");
            let original = admin.join(name);
            if original.exists() {
                fs::remove_file(&original).unwrap();
            }
            let outside = tempfile::NamedTempFile::new().unwrap();
            if hardlink {
                fs::hard_link(outside.path(), &original).unwrap();
            } else {
                std::os::unix::fs::symlink(outside.path(), &original).unwrap();
            }
            assert!(Comb::rebuild(root.path()).is_err());
            assert!(!admin.join(INTENT).exists());
            assert!(fs::symlink_metadata(original).is_ok());
            assert_eq!(outside.as_file().metadata().unwrap().len(), 0);
        }
    }
}

#[test]
fn shared_write_candidates_refuse_before_acceptance() {
    use std::os::unix::fs::PermissionsExt;
    for name in NAMES {
        let root = fixture(true);
        let admin = root.path().join(".comb");
        let path = admin.join(name);
        let original = fs::read(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o620)).unwrap();
        assert_eq!(
            Comb::rebuild(root.path()).err().unwrap().code,
            crate::ErrorCode::UnsafePath
        );
        assert!(!admin.join(INTENT).exists());
        assert_eq!(fs::read(path).unwrap(), original);
    }
}

#[test]
fn documented_manual_vector_cache_disposal_rebuilds_without_losing_knowledge() {
    let root = fixture(true);
    let admin = root.path().join(".comb");
    for name in &NAMES[4..] {
        fs::remove_file(admin.join(name)).unwrap();
    }
    drop(Comb::rebuild(root.path()).unwrap());
    recover(root.path());
    assert!(admin.join("vectors.sqlite").exists());
}

#[test]
fn explicit_rebuild_replaces_invalid_vectors_but_keeps_valid_cache_bytes() {
    for contents in [
        None,
        Some(b"".as_slice()),
        Some(b"not a database".as_slice()),
    ] {
        let root = fixture(false);
        let cache = root.path().join(".comb/vectors.sqlite");
        let before = fs::read(&cache).unwrap();
        if let Some(contents) = contents {
            atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
                .write(&cache, contents)
                .unwrap();
        }
        drop(Comb::rebuild(root.path()).unwrap());
        if contents.is_none() {
            assert_eq!(fs::read(cache).unwrap(), before);
        }
        recover(root.path());
    }
    let root = fixture(false);
    let cache = root.path().join(".comb/vectors.sqlite");
    let db = rusqlite::Connection::open(&cache).unwrap();
    db.execute_batch("CREATE TABLE unexpected(value)").unwrap();
    drop(db);
    assert!(Comb::open(root.path()).is_err());
    drop(Comb::rebuild(root.path()).unwrap());
    recover(root.path());
}

#[test]
fn later_schema_failure_keeps_accepted_rebuild_commit_status() {
    let root = fixture(false);
    let error = interrupt(root.path(), "index.sqlite", Step::Written, 1);
    assert!(error.file_committed);
    assert!(!root.path().join(".comb/index.sqlite").exists());
    assert!(root.path().join(".comb").join(POPULATION).exists());
    recover(root.path());
}

#[test]
fn native_retry_barrier_errors_preserve_accepted_state_and_prevent_new_sqlite_admission() {
    use crate::disk::{Operation, Recording};
    use std::sync::Arc;
    for handoff in [false, true] {
        for (operation, occurrence) in [
            (Operation::Fsync, 1),
            (Operation::Fsync, 2),
            (Operation::FullFlush, 1),
        ] {
            let root = fixture(true);
            if handoff {
                interrupt(root.path(), POPULATION, Step::BeforeFileSync, 3);
            } else {
                interrupt(root.path(), INTENT, Step::AfterPublish, 1);
            }
            let main = root.path().join(".comb/index.sqlite");
            let before = crate::test_support::optional_bytes(&main);
            let mut disk = Disk::acquire(root.path(), false).unwrap();
            let durability = Arc::new(Recording::default());
            disk.set_durability(durability.clone());
            durability.fail(operation, occurrence);
            let error = disk.rebuild_index(false).unwrap_err();
            assert!(error.file_committed);
            assert_eq!(error.code, crate::ErrorCode::IoError);
            assert_eq!(crate::test_support::optional_bytes(&main), before);
            drop(disk);
            recover(root.path());
        }
    }
}

#[test]
fn actual_final_handoff_full_flush_failure_resumes_through_population_only_state() {
    let root = fixture(true);
    interrupt(root.path(), INTENT, Step::AfterPublish, 1);
    let mut disk = Disk::acquire(root.path(), false).unwrap();
    let durability = std::sync::Arc::new(crate::disk::Recording::default());
    durability.barrier(Ok(()));
    disk.set_durability(durability.clone());
    durability.fail(crate::disk::Operation::FullFlush, 4);
    assert!(disk.rebuild_index(false).unwrap_err().file_committed);
    assert!(!disk.admin.join(INTENT).exists());
    assert!(!disk.index_path().exists());
    assert!(disk.admin.join(POPULATION).exists());
    drop(disk);
    recover(root.path());
}
