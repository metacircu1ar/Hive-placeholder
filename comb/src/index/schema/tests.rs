use super::*;
use crate::disk::publication_hooks as hooks;
use atomic_fs::Step;
use std::{fs, io, os::unix::fs::MetadataExt, path::PathBuf};

mod concurrency;
mod process;

#[test]
fn compatibility_schemas_match_native_object_contracts() {
    for name in NAMES {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(definition(name)).unwrap();
        crate::golden::json(&format!("{name}-schema.json"), &schema(&db).unwrap());
    }
}

const STEPS: [Step; 7] = [
    Step::Written,
    Step::BeforeFileSync,
    Step::AfterFileSync,
    Step::BeforePublish,
    Step::AfterPublish,
    Step::BeforeDirectorySync,
    Step::AfterDirectorySync,
];
fn directory() -> tempfile::TempDir {
    // Disk canonicalizes knowledge roots before constructing native DB paths.
    // Match that contract: macOS /var itself is an ancestor symlink.
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}
const NAMES: [&str; 2] = ["index.sqlite", "vectors.sqlite"];
fn definition(name: &str) -> &'static str {
    if name == "index.sqlite" {
        SCHEMA
    } else {
        include_str!("../vector_schema.sql")
    }
}
fn missing_target(root: &Path, name: &str) -> PathBuf {
    if name == "vectors.sqlite" {
        drop(Index::open(&root.join("index.sqlite")).unwrap());
        fs::remove_file(root.join(name)).unwrap();
    }
    root.join(name)
}
fn complete(path: &Path, name: &str) {
    assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o600);
    let db = readonly(path).unwrap();
    check_schema(&db, path, definition(name)).unwrap();
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}
fn no_sidecars(path: &Path) {
    for suffix in ["-wal", "-shm", "-journal"] {
        assert!(!companion(path, suffix).exists());
    }
}
fn retry(root: &Path) {
    let db = Index::open(&root.join("index.sqlite")).unwrap();
    for (name, expected) in [("synchronous", 2), ("foreign_keys", 1), ("fullfsync", 0)] {
        assert_eq!(
            db.connection
                .pragma_query_value(None, name, |r| r.get::<_, i64>(0))
                .unwrap(),
            expected
        );
    }
    assert_eq!(
        db.connection
            .pragma_query_value(None, "journal_mode", |r| r.get::<_, String>(0))
            .unwrap(),
        "wal"
    );
    drop(db);
    for name in NAMES {
        complete(&root.join(name), name);
    }
}

#[test]
fn both_schema_publications_keep_complete_versions_native_causes_and_retry() {
    use std::error::Error as _;
    for name in NAMES {
        for (position, at) in STEPS.into_iter().enumerate() {
            let directory = directory();
            let target = missing_target(directory.path(), name);
            let error = hooks::with(
                move |path, step| {
                    if path.file_name().unwrap() == name && step == at {
                        return Err(io::Error::from_raw_os_error(libc::EIO));
                    }
                    Ok(())
                },
                || Index::open(&directory.path().join("index.sqlite")),
            )
            .err()
            .unwrap();
            assert_eq!(error.file_committed, position >= 4);
            let cause = error
                .source()
                .unwrap()
                .downcast_ref::<atomic_fs::Error>()
                .unwrap();
            assert_eq!(cause.io_error().raw_os_error(), Some(libc::EIO));
            assert_eq!(target.exists(), position >= 4);
            no_sidecars(&target);
            if target.exists() {
                complete(&target, name);
            }
            retry(directory.path());
        }
    }
}

#[test]
fn retry_repeats_parent_sync_and_keeps_the_same_published_schema() {
    for name in NAMES {
        let directory = directory();
        let target = missing_target(directory.path(), name);
        let mut before = None;
        for _ in 0..2 {
            let error = hooks::with(
                move |path, step| {
                    if path.file_name().unwrap() == name && step == Step::BeforeDirectorySync {
                        Err(io::Error::from_raw_os_error(libc::EIO))
                    } else {
                        Ok(())
                    }
                },
                || Index::open(&directory.path().join("index.sqlite")),
            )
            .err()
            .unwrap();
            assert!(error.file_committed);
            complete(&target, name);
            let bytes = fs::read(&target).unwrap();
            if let Some(before) = &before {
                assert_eq!(&bytes, before);
            } else {
                before = Some(bytes);
            }
            no_sidecars(&target);
        }
        retry(directory.path());
    }
}

#[test]
fn private_preparations_do_not_clobber_a_no_replace_winner() {
    for name in NAMES {
        let directory = directory();
        let target = missing_target(directory.path(), name);
        let memory = Connection::open_in_memory().unwrap();
        memory.execute_batch(definition(name)).unwrap();
        let winner = memory.serialize(rusqlite::MAIN_DB).unwrap().to_vec();
        let saved = winner.clone();
        let error = hooks::with(
            move |path, step| {
                if path.file_name().unwrap() != name {
                    return Ok(());
                }
                if step == Step::Written {
                    assert!(!path.exists());
                    for entry in fs::read_dir(path.parent().unwrap())? {
                        let entry = entry?;
                        if entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with(".comb-tmp-")
                        {
                            assert_eq!(entry.metadata()?.mode() & 0o777, 0o600);
                        }
                    }
                }
                if step == Step::BeforePublish {
                    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
                        .create_new(path, &winner)
                        .map_err(io::Error::other)?;
                }
                Ok(())
            },
            || Index::open(&directory.path().join("index.sqlite")),
        )
        .err()
        .unwrap();
        assert!(!error.file_committed);
        assert_eq!(fs::read(&target).unwrap(), saved);
        complete(&target, name);
        retry(directory.path());
    }
}

#[test]
fn empty_and_partial_existing_databases_are_rejected_before_configuration() {
    for name in NAMES {
        for partial in [false, true] {
            let directory = directory();
            let target = missing_target(directory.path(), name);
            atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
                .create_new(&target, b"")
                .unwrap();
            if partial {
                let db = Connection::open(&target).unwrap();
                db.execute_batch("CREATE TABLE partial(value)").unwrap();
            }
            let before = fs::read(&target).unwrap();
            let error = Index::open(&directory.path().join("index.sqlite"))
                .err()
                .unwrap();
            assert_eq!(error.code, crate::ErrorCode::UnsupportedIndex);
            assert!(!error.file_committed);
            assert_eq!(fs::read(&target).unwrap(), before);
            no_sidecars(&target);
        }
    }
}

#[test]
fn orphan_sidecars_and_unsafe_journals_are_never_adopted() {
    for name in NAMES {
        for suffix in ["-wal", "-shm", "-journal"] {
            let directory = directory();
            let target = missing_target(directory.path(), name);
            let sidecar = companion(&target, suffix);
            atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
                .create_new(&sidecar, b"orphan")
                .unwrap();
            assert!(Index::open(&directory.path().join("index.sqlite")).is_err());
            assert!(!target.exists());
            assert_eq!(fs::read(sidecar).unwrap(), b"orphan");
        }
    }
    let directory = directory();
    retry(directory.path());
    let outside = tempfile::NamedTempFile::new().unwrap();
    for name in NAMES {
        let journal = companion(&directory.path().join(name), "-journal");
        std::os::unix::fs::symlink(outside.path(), &journal).unwrap();
        assert_eq!(
            Index::open(&directory.path().join("index.sqlite"))
                .err()
                .unwrap()
                .code,
            crate::ErrorCode::UnsafePath
        );
        fs::remove_file(journal).unwrap();
    }
}

#[test]
fn vector_publication_stays_committed_when_subsequent_attachment_safety_check_fails() {
    let directory = directory();
    let target = missing_target(directory.path(), "vectors.sqlite");
    let main = directory.path().join("index.sqlite");
    let moved = directory.path().join("moved.sqlite");
    let observed = main.clone();
    let replacement = moved.clone();
    let error = hooks::with(
        move |path, step| {
            if path.file_name().unwrap() == "vectors.sqlite" && step == Step::AfterDirectorySync {
                fs::rename(&observed, &replacement)?;
            }
            Ok(())
        },
        || Index::open(&main),
    )
    .err()
    .unwrap();
    assert!(error.file_committed);
    assert!(!main.exists());
    complete(&target, "vectors.sqlite");
    fs::rename(moved, main).unwrap();
    retry(directory.path());
}

#[test]
fn postpublication_sqlite_open_never_recreates_a_missing_file_and_keeps_commit_state() {
    for name in NAMES {
        let directory = directory();
        let target = missing_target(directory.path(), name);
        let moved = directory.path().join("published.sqlite");
        let replacement = moved.clone();
        let error = hooks::with(
            move |path, step| {
                if path.file_name().unwrap() == name && step == Step::AfterDirectorySync {
                    fs::rename(path, &replacement)?;
                }
                Ok(())
            },
            || Index::open(&directory.path().join("index.sqlite")),
        )
        .err()
        .unwrap();
        assert!(error.file_committed);
        assert!(!target.exists());
        complete(&moved, name);
        fs::rename(moved, target).unwrap();
        retry(directory.path());
    }
}

#[test]
fn empty_main_rejection_preserves_existing_native_wal_evidence() {
    for name in NAMES {
        let directory = directory();
        let target = missing_target(directory.path(), name);
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .create_new(&target, b"")
            .unwrap();
        let wal = companion(&target, "-wal");
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .create_new(&wal, b"owned WAL evidence")
            .unwrap();
        assert_eq!(
            Index::open(&directory.path().join("index.sqlite"))
                .err()
                .unwrap()
                .code,
            crate::ErrorCode::UnsupportedIndex
        );
        assert!(fs::read(&target).unwrap().is_empty());
        assert_eq!(fs::read(&wal).unwrap(), b"owned WAL evidence");
        assert!(!companion(&target, "-shm").exists());
        assert!(!companion(&target, "-journal").exists());
    }
}

#[test]
fn rejected_wal_schema_keeps_main_bytes_but_sqlite_may_create_private_sidecars() {
    for name in NAMES {
        let directory = directory();
        let target = missing_target(directory.path(), name);
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .create_new(&target, b"")
            .unwrap();
        {
            let db = Connection::open(&target).unwrap();
            db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE partial(value)")
                .unwrap();
        }
        no_sidecars(&target);
        let before = fs::read(&target).unwrap();
        assert_eq!(
            Index::open(&directory.path().join("index.sqlite"))
                .err()
                .unwrap()
                .code,
            crate::ErrorCode::UnsupportedIndex
        );
        assert_eq!(fs::read(&target).unwrap(), before);
        // These are SQLite's own inspection artifacts, not a Hive initialization.
        let wal = companion(&target, "-wal");
        assert!(fs::read(&wal).unwrap().is_empty());
        for suffix in ["-wal", "-shm"] {
            let meta = fs::symlink_metadata(companion(&target, suffix)).unwrap();
            disk::check_file(&meta).unwrap();
            assert_eq!(meta.mode() & 0o777, 0o600);
        }
    }
}

#[test]
fn explicit_rebuild_still_recovers_empty_or_missing_main_and_preserves_valid_vectors() {
    for empty in [true, false] {
        let directory = directory();
        drop(crate::Comb::init(directory.path()).unwrap());
        let admin = directory.path().join(".comb");
        let main = admin.join("index.sqlite");
        let vectors = admin.join("vectors.sqlite");
        let saved = fs::read(&vectors).unwrap();
        fs::remove_file(&main).unwrap();
        if empty {
            atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
                .create_new(&main, b"")
                .unwrap();
        }
        // Explicit rebuild already owns removal of these checked index sidecars.
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .create_new(companion(&main, "-wal"), b"old WAL evidence")
            .unwrap();
        drop(crate::Comb::rebuild(directory.path()).unwrap());
        complete(&main, "index.sqlite");
        assert_eq!(fs::read(vectors).unwrap(), saved);
        assert!(!admin.join("index-population.json").exists());
    }
}
