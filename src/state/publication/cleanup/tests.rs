use super::*;
use crate::config::{Method, Profile, Provider};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
};

fn fixture() -> (tempfile::TempDir, State, Lock) {
    let temp = crate::runtime::test_directory();
    let state = State::new(temp.path().join("state")).unwrap();
    state
        .register(
            "mock",
            &Profile {
                provider: Provider::Codex,
                method: Method::Subscription,
                api_key_env: None,
                api_key_file: None,
            },
        )
        .unwrap();
    state::directory(&state.root.join("runtime")).unwrap();
    state::directory(&state.root.join("runtime/output")).unwrap();
    let owner = Lock::acquire(&state.root.join("runtime/daemon.lock")).unwrap();
    (temp, state, owner)
}

fn preparation(dir: &Path) -> std::path::PathBuf {
    let path = dir.join(".hive-tmp-state-ABC123");
    state::private_file(&path, true).unwrap();
    path
}

#[test]
fn startup_removes_only_dedicated_files_and_does_not_recurse() {
    let (_temp, state, owner) = fixture();
    let mut removed = Vec::new();
    let mut retained = Vec::new();
    for directory in [
        &state.root,
        &state.root.join("runtime"),
        &state.root.join("runtime/output"),
    ] {
        for name in [".hive-tmp-state-ABC123", ".hive-tmp-state-0123456789abcdef"] {
            let path = directory.join(name);
            state::private_file(&path, true).unwrap();
            removed.push(path);
        }
        for name in [
            ".hive-tmp-ABC123",
            ".hive-tmp-0123456789abcdef",
            ".comb-tmp-ABC123",
            ".tmpABC123",
            ".hive-tmp-state-ABCDE",
            "committed.json",
        ] {
            let path = directory.join(name);
            state::write_json(&path, &serde_json::json!({"keep":true})).unwrap();
            retained.push(path);
        }
    }
    let nested = state.root.join("runtime/output/nested");
    state::directory(&nested).unwrap();
    retained.push(preparation(&nested));
    let snapshots: Vec<_> = retained
        .iter()
        .map(|path| fs::read(path).unwrap())
        .collect();
    state.cleanup_preparations(&owner).unwrap();
    assert!(removed.iter().all(|path| !path.exists()));
    for (path, expected) in retained.iter().zip(snapshots) {
        assert_eq!(fs::read(path).unwrap(), expected);
    }
    state.cleanup_preparations(&owner).unwrap();
}

#[test]
fn errors_at_each_removal_boundary_are_recoverable_and_empty_retry_syncs() {
    for at in [
        Step::BeforeRemove,
        Step::AfterRemove,
        Step::BeforeDirectorySync,
        Step::AfterDirectorySync,
    ] {
        let (_temp, state, owner) = fixture();
        let directory = state.root.join("runtime/output");
        let path = preparation(&directory);
        let target = if matches!(at, Step::BeforeRemove | Step::AfterRemove) {
            path.clone()
        } else {
            directory.clone()
        };
        let error = state::sync_tests::with_hook(
            move |path, step| {
                if path == target && step == at {
                    Err(io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
            || state.cleanup_preparations(&owner),
        )
        .unwrap_err();
        assert_eq!(error.io_raw_os_error(), Some(libc::EIO));
        assert_eq!(path.exists(), at == Step::BeforeRemove);
        let observed = Arc::new(Mutex::new(Vec::new()));
        let events = observed.clone();
        state::sync_tests::with_hook(
            move |path, step| {
                if path == directory {
                    events.lock().unwrap().push(step);
                }
                Ok(())
            },
            || state.cleanup_preparations(&owner),
        )
        .unwrap();
        assert_eq!(
            *observed.lock().unwrap(),
            [Step::BeforeDirectorySync, Step::AfterDirectorySync]
        );
        assert!(!path.exists());
    }
}

#[test]
fn unsafe_candidates_are_retained_without_following_them() {
    for kind in ["public", "directory", "symlink", "hardlink"] {
        let (_temp, state, owner) = fixture();
        let dir = state.root.join("runtime/output");
        let saved = dir.join("saved.json");
        state::write_json(&saved, &serde_json::json!({"sentinel":true})).unwrap();
        let bytes = fs::read(&saved).unwrap();
        let candidate = dir.join(".hive-tmp-state-ABC123");
        match kind {
            "public" => {
                preparation(&dir);
                fs::set_permissions(&candidate, fs::Permissions::from_mode(0o644)).unwrap();
            }
            "directory" => state::directory(&candidate).unwrap(),
            "symlink" => std::os::unix::fs::symlink(&saved, &candidate).unwrap(),
            "hardlink" => fs::hard_link(&saved, &candidate).unwrap(),
            _ => unreachable!(),
        }
        assert_eq!(
            state.cleanup_preparations(&owner).unwrap_err().code(),
            "unsafe_state"
        );
        assert!(candidate.symlink_metadata().is_ok());
        assert_eq!(fs::read(&saved).unwrap(), bytes);
    }
}

#[test]
fn replaced_entry_or_parent_cannot_redirect_a_pending_unlink() {
    for parent in [false, true] {
        let (_temp, state, owner) = fixture();
        let directory = state.root.join("runtime/output");
        let candidate = preparation(&directory);
        let target = candidate.clone();
        let moved = directory.with_file_name("old-output");
        let old = moved.clone();
        state::sync_tests::with_hook(
            move |path, step| {
                if path == target && step == Step::BeforeRemove {
                    if parent {
                        fs::rename(&directory, &old).unwrap();
                        state::directory(&directory).unwrap();
                    } else {
                        fs::rename(path, path.with_file_name("saved-preparation")).unwrap();
                    }
                    state::private_file(path, true).unwrap();
                }
                Ok(())
            },
            || {
                assert_eq!(
                    state.cleanup_preparations(&owner).unwrap_err().code(),
                    "unsafe_state"
                );
            },
        );
        assert!(candidate.exists());
        if parent {
            assert!(moved.join(candidate.file_name().unwrap()).exists());
        }
    }
}

mod concurrency;
mod process;

#[test]
fn invalid_formats_and_replaced_locks_authorize_no_cleanup() {
    for kind in ["index", "sqlite", "daemon-lock", "index-lock"] {
        let (_temp, state, owner) = fixture();
        let candidate = preparation(&state.root);
        match kind {
            "index" => state::write_json(
                &state.root.join("index.json"),
                &serde_json::json!({"unsupported":true}),
            )
            .unwrap(),
            "sqlite" => {
                atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
                    .create_new(state.root.join("runtime/runtime.sqlite"), b"")
                    .unwrap();
            }
            "daemon-lock" => {
                let path = state.root.join("runtime/daemon.lock");
                fs::rename(&path, path.with_extension("saved")).unwrap();
                state::private_file(&path, true).unwrap();
            }
            _ => {}
        }
        let index = state.root.join("index.lock");
        let replace = kind == "index-lock";
        let result = state::sync_tests::with_hook(
            move |path, step| {
                if replace && step == Step::BeforeRemove {
                    fs::rename(&index, index.with_extension("saved")).unwrap();
                    state::private_file(&index, true).unwrap();
                    assert!(
                        path.file_name()
                            .is_some_and(hive_setup_support::names::state_preparation_name)
                    );
                }
                Ok(())
            },
            || state.cleanup_preparations(&owner),
        );
        assert!(result.is_err(), "{kind}");
        assert!(candidate.exists(), "{kind}");
    }
}
