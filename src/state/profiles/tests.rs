use super::*;
use crate::config::{Method, Provider};

#[test]
fn pending_binding_allows_only_its_genuinely_missing_location() {
    let tmp = tempfile::tempdir().unwrap();
    let state = State::new(tmp.path().join("state")).unwrap();
    drop(state.initialize().unwrap());
    let id = Uuid::new_v4();
    let mut binding = Binding {
        id,
        profile: Profile {
            provider: Provider::Codex,
            method: Method::Subscription,
            api_key_env: None,
            api_key_file: None,
        },
        ready: false,
        directory: state.root.join("profiles").join(id.to_string()),
    };
    state.validate_binding(&binding).unwrap();
    assert!(!binding.directory.exists());
    binding.ready = true;
    assert!(state.validate_binding(&binding).is_err());
    binding.ready = false;
    binding.directory = tmp.path().join("another-missing-location");
    assert!(state.validate_binding(&binding).is_err());
}

#[test]
fn binding_resolution_preserves_io_failure_instead_of_comparing_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let state = State::new(tmp.path().join("state")).unwrap();
    drop(state.initialize().unwrap());
    directory(&state.root.join("profiles")).unwrap();
    let id = Uuid::new_v4();
    let binding = Binding {
        id,
        profile: Profile {
            provider: Provider::Codex,
            method: Method::Subscription,
            api_key_env: None,
            api_key_file: None,
        },
        ready: false,
        directory: state.root.join("profiles").join(id.to_string()),
    };
    std::os::unix::fs::symlink(&binding.directory, &binding.directory).unwrap();
    let error = state.validate_binding(&binding).unwrap_err();
    assert_eq!(error.code(), "state_io");
    assert_eq!(error.io_raw_os_error(), Some(libc::ELOOP));
}

#[test]
fn independent_index_transactions_wait_instead_of_reporting_profile_busy() {
    let tmp = tempfile::tempdir().unwrap();
    let state = State::new(tmp.path().join("state")).unwrap();
    let profile = Profile {
        provider: Provider::Codex,
        method: Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    state.register("base", &profile).unwrap();
    std::thread::scope(|scope| {
        for i in 0..8 {
            let state = &state;
            let profile = &profile;
            scope.spawn(move || {
                state.register(&format!("p{i}"), profile).unwrap();
                for _ in 0..10 {
                    assert!(state.lookup("base", profile).unwrap().is_some());
                }
            });
        }
    });
}
#[test]
fn interrupted_registration_resumes_without_changing_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let state = State::new(tmp.path().join("state")).unwrap();
    drop(state.initialize().unwrap());
    let profile = Profile {
        provider: Provider::Claude,
        method: Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    let id = Uuid::new_v4();
    let pending = Binding {
        id,
        profile: profile.clone(),
        ready: false,
        directory: state.root.join("profiles").join(id.to_string()),
    };
    let mut index = Index::default();
    index.generations.insert(
        id,
        super::super::generations::Generation::new(
            pending.clone(),
            "main",
            Some("test-user".into()),
        ),
    );
    index.profiles.insert("main".into(), pending.clone());
    write_json(&state.root.join("index.json"), &index).unwrap();
    directory(&state.root.join("profiles")).unwrap();
    directory(&pending.directory).unwrap();
    // Simulate an interruption after mkdir, before identity/provider/index-ready writes.
    assert!(!state.lookup("main", &profile).unwrap().unwrap().ready);
    let complete = state.register("main", &profile).unwrap();
    assert_eq!(complete.id, pending.id);
    assert!(complete.ready);
    fs::write(state.root.join("profiles/.DS_Store"), "metadata").unwrap();
    assert!(state.lookup("main", &profile).unwrap().unwrap().ready);
}
#[test]
fn symlinked_root_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(tmp.path(), tmp.path().join("link")).unwrap();
    let state = State::new(tmp.path().join("link")).unwrap();
    assert_eq!(state.read_index().err().unwrap().code(), "unsafe_state");
}
#[test]
fn indexless_database_validation_preserves_wal_and_rejects_unknown_formats() {
    for (defect, supported) in [
        ("empty", false),
        ("missing", false),
        ("current", true),
        ("unknown", false),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let state = State::new(tmp.path().join("state")).unwrap();
        drop(state.initialize().unwrap());
        let runtime = state.root.join("runtime");
        directory(&runtime).unwrap();
        let path = runtime.join("runtime.sqlite");
        super::super::private_file(&path, true).unwrap();
        let db = rusqlite::Connection::open(&path).unwrap();
        if defect != "empty" {
            db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
                .unwrap();
            crate::runtime::initialize_schema(&db).unwrap();
            match defect {
                "missing" => db
                    .execute_batch("DROP TRIGGER messages_ready_insert")
                    .unwrap(),
                "unknown" => db
                    .execute_batch("CREATE TABLE unexpected(value TEXT)")
                    .unwrap(),
                _ => {}
            }
        }
        // Remove SHM while this test retains the only source connection.
        // Inspection may rebuild volatile coordination files, never history.
        let shm = runtime.join("runtime.sqlite-shm");
        if shm.exists() {
            fs::remove_file(&shm).unwrap();
        }
        let bytes = |name| crate::test_support::optional_bytes(runtime.join(name));
        let before = (bytes("runtime.sqlite"), bytes("runtime.sqlite-wal"));
        let result = state.check_format();
        assert_eq!(result.is_ok(), supported, "{result:?}");
        assert_eq!(
            (bytes("runtime.sqlite"), bytes("runtime.sqlite-wal")),
            before
        );
        // SQLite may recreate SHM while reading the unchanged WAL.
        assert!(!state.root.join("index.json").exists());
        if !supported {
            assert_eq!(
                crate::runtime::check_existing_format(&path)
                    .unwrap_err()
                    .code(),
                "unsupported_schema"
            );
            assert_eq!(
                (bytes("runtime.sqlite"), bytes("runtime.sqlite-wal")),
                before
            );
        }
    }
}
#[test]
fn oversized_current_index_remains_a_corruption_error() {
    let tmp = tempfile::tempdir().unwrap();
    let state = State::new(tmp.path().join("state")).unwrap();
    drop(state.initialize().unwrap());
    write_json(
        &state.root.join("index.json"),
        &serde_json::json!({"payload":"x".repeat(super::super::STATE_RECORD_BYTES)}),
    )
    .unwrap();
    assert_eq!(
        state.check_format().unwrap_err().code(),
        "state_inconsistent"
    );
}

#[test]
fn current_root_marker_and_newer_database_disagreement_is_damage() {
    for indexed in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let state = State::new(temp.path().join("state")).unwrap();
        drop(state.initialize().unwrap());
        if indexed {
            write_json(&state.root.join("index.json"), &Index::default()).unwrap();
        }
        let runtime = state.root.join("runtime");
        directory(&runtime).unwrap();
        let path = runtime.join("runtime.sqlite");
        drop(super::super::private_file(&path, true).unwrap());
        let db = rusqlite::Connection::open(&path).unwrap();
        crate::runtime::initialize_schema(&db).unwrap();
        db.pragma_update(None, "user_version", 2).unwrap();
        let before = db.serialize(rusqlite::MAIN_DB).unwrap().to_vec();
        let error = state.check_format().unwrap_err();
        assert_eq!(error.code(), "unsupported_schema");
        assert!(error.message().contains("disagree"));
        assert_eq!(&*db.serialize(rusqlite::MAIN_DB).unwrap(), before);
    }
}
#[test]
fn indexless_missing_database_never_adopts_orphan_sidecars() {
    for suffix in ["-wal", "-journal", "-shm"] {
        let tmp = tempfile::tempdir().unwrap();
        let state = State::new(tmp.path().join("state")).unwrap();
        drop(state.initialize().unwrap());
        let runtime = state.root.join("runtime");
        directory(&runtime).unwrap();
        let path = runtime.join(format!("runtime.sqlite{suffix}"));
        std::os::unix::fs::symlink(tmp.path().join("missing"), &path).unwrap();
        assert_eq!(
            state.check_format().unwrap_err().code(),
            "recovery_required"
        );
        assert!(path.symlink_metadata().is_ok());
        assert!(!runtime.join("runtime.sqlite").exists());
        assert!(!state.root.join("index.json").exists());
    }
    let tmp = tempfile::tempdir().unwrap();
    let state = State::new(tmp.path().join("state")).unwrap();
    drop(state.initialize().unwrap());
    std::os::unix::fs::symlink(tmp.path().join("missing"), state.root.join("runtime")).unwrap();
    assert_eq!(state.check_format().unwrap_err().code(), "unsafe_state");
}
#[test]
fn registration_reads_an_index_replaced_while_it_is_opened() {
    let tmp = tempfile::tempdir().unwrap();
    let state = State::new(tmp.path().join("state")).unwrap();
    let profile = Profile {
        provider: Provider::Codex,
        method: Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    state.register("base", &profile).unwrap();
    let replaced = std::rc::Rc::new(std::cell::Cell::new(false));
    let once = replaced.clone();
    let registered = crate::state::sync_tests::with_checkpoint(
        move |path, at| {
            if at == hive_setup_support::checkpoints::Checkpoint::RecordOpened
                && path.file_name() == Some("index.json".as_ref())
                && !once.replace(true)
            {
                // A concurrent writer renames an identical index over the open one.
                let current: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(path)?).unwrap();
                write_json(path, &current).unwrap();
            }
            Ok(())
        },
        || state.register("next", &profile),
    );
    registered.unwrap();
    assert!(replaced.get());
    assert!(state.lookup("next", &profile).unwrap().is_some());
}
#[test]
fn a_locked_index_read_refuses_a_replaced_index() {
    let tmp = tempfile::tempdir().unwrap();
    let state = State::new(tmp.path().join("state")).unwrap();
    let profile = Profile {
        provider: Provider::Codex,
        method: Method::Subscription,
        api_key_env: None,
        api_key_file: None,
    };
    state.register("base", &profile).unwrap();
    let read = crate::state::sync_tests::with_checkpoint(
        |path, at| {
            if at == hive_setup_support::checkpoints::Checkpoint::RecordOpened {
                let current: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(path)?).unwrap();
                write_json(path, &current).unwrap();
            }
            Ok(())
        },
        || state.read_index().map(drop),
    );
    assert_eq!(read.unwrap_err().code(), "unsafe_state");
}
