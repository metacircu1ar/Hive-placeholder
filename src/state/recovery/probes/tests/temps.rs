use super::*;
use crate::state::sync_tests::with_hook;
use atomic_fs::Step;
use std::{
    io,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{Arc, Mutex},
};
mod death;

const STEPS: [Step; 4] = [
    Step::BeforeRemove,
    Step::AfterRemove,
    Step::BeforeDirectorySync,
    Step::AfterDirectorySync,
];

fn fixture() -> (tempfile::TempDir, State) {
    let tmp = tempfile::tempdir().unwrap();
    let state = State::new(tmp.path().join("state")).unwrap();
    drop(state.initialize().unwrap());
    state::directory(&state.root.join("native-probes")).unwrap();
    (tmp, state)
}
pub(super) fn stages(state: &State) -> Vec<PathBuf> {
    fs::read_dir(state.root.join("native-probes"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .is_some_and(hive_setup_support::names::state_preparation_name)
        })
        .collect()
}
fn preparation(state: &State) -> PathBuf {
    let path = state.root.join("native-probes/.hive-tmp-state-ABC123");
    state::private_file(&path, true).unwrap();
    path
}

#[test]
fn only_immediate_dedicated_preparations_are_removed() {
    let (_tmp, state, operation, dir) = preparation::fixture();
    let base = state.root.join("native-probes");
    let mut retained = vec![
        dir.join(".hive-tmp-state-ABC123"),
        state.root.join(".hive-tmp-state-ABC123"),
    ];
    for name in [
        ".hive-tmp-ABC123",
        ".hive-tmp-0123456789abcdef",
        ".hive-tmp-state-short",
        ".comb-tmp-ABC123",
    ] {
        retained.push(base.join(name));
    }
    // A committed but invalid permit remains for diagnosis; a temp isn't authority.
    retained.push(base.join(format!("{}.prune.json", Uuid::new_v4())));
    for path in &retained {
        write_json(path, &serde_json::json!({"sentinel":true})).unwrap();
    }
    let original = fs::read(dir.join("identity.json")).unwrap();
    preparation(&state);
    state::private_file(&base.join(".hive-tmp-state-0123456789abcdef"), true).unwrap();
    state.sweep_metadata_probes_checked().unwrap();
    assert!(stages(&state).is_empty());
    assert_eq!(fs::read(dir.join("identity.json")).unwrap(), original);
    for path in retained {
        assert_eq!(fs::read(path).unwrap(), b"{\"sentinel\":true}\n");
    }
    drop(operation);
}

#[test]
fn cleanup_faults_preserve_retry_and_sync_even_when_empty() {
    for at in STEPS {
        let (_tmp, state) = fixture();
        let path = preparation(&state);
        let base = state.root.join("native-probes");
        let target = if matches!(at, Step::BeforeRemove | Step::AfterRemove) {
            path.clone()
        } else {
            base.clone()
        };
        let error = with_hook(
            move |path, step| {
                if path == target && step == at {
                    Err(io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
            || state.sweep_metadata_probes_checked(),
        )
        .unwrap_err();
        assert_eq!(error.io_raw_os_error(), Some(libc::EIO));
        assert_eq!(path.exists(), at == Step::BeforeRemove);
        let observed = Arc::new(Mutex::new(Vec::new()));
        let events = observed.clone();
        with_hook(
            move |path, step| {
                if path == base {
                    events.lock().unwrap().push(step);
                }
                Ok(())
            },
            || state.sweep_metadata_probes_checked(),
        )
        .unwrap();
        // The preparation sweep syncs first, then the existing permit-removal retry.
        assert_eq!(
            *observed.lock().unwrap(),
            [
                Step::BeforeDirectorySync,
                Step::AfterDirectorySync,
                Step::BeforeDirectorySync,
                Step::AfterDirectorySync,
            ]
        );
        assert!(!path.exists());
    }
}

#[test]
fn unsafe_entries_fail_closed_and_best_effort_api_keeps_them() {
    for kind in ["public", "directory", "symlink", "hardlink"] {
        let (_tmp, state) = fixture();
        let base = state.root.join("native-probes");
        let saved = base.join("saved");
        write_json(&saved, &serde_json::json!({"sentinel":true})).unwrap();
        let candidate = base.join(".hive-tmp-state-ABC123");
        match kind {
            "public" => {
                preparation(&state);
                fs::set_permissions(&candidate, fs::Permissions::from_mode(0o644)).unwrap();
            }
            "directory" => state::directory(&candidate).unwrap(),
            "symlink" => std::os::unix::fs::symlink(&saved, &candidate).unwrap(),
            "hardlink" => fs::hard_link(&saved, &candidate).unwrap(),
            _ => unreachable!(),
        }
        assert_eq!(
            state.sweep_metadata_probes_checked().unwrap_err().code(),
            "unsafe_state"
        );
        state.sweep_metadata_probes();
        assert!(candidate.symlink_metadata().is_ok());
        assert_eq!(fs::read(&saved).unwrap(), b"{\"sentinel\":true}\n");
    }
}

#[test]
fn replaced_lock_cannot_authorize_unlink_or_directory_sync() {
    let _fork = crate::os::fork_guard().unwrap();
    for at in [Step::BeforeRemove, Step::BeforeDirectorySync] {
        let (_tmp, state) = fixture();
        let candidate = preparation(&state);
        let lock = state.root.join("native-probes.lock");
        let error = with_hook(
            move |_, step| {
                if step == at {
                    fs::rename(&lock, lock.with_extension("saved"))?;
                    state::private_file(&lock, true).unwrap();
                }
                Ok(())
            },
            || state.sweep_metadata_probes_checked(),
        )
        .unwrap_err();
        assert_eq!(error.code(), "unsafe_state");
        assert_eq!(candidate.exists(), at == Step::BeforeRemove);
        state.sweep_metadata_probes_checked().unwrap();
        assert!(!candidate.exists());
    }
}

#[test]
fn unsupported_index_and_unsafe_parent_precede_temp_deletion() {
    for index in [true, false] {
        let (_tmp, state) = fixture();
        let candidate = preparation(&state);
        if index {
            write_json(
                &state.root.join("index.json"),
                &serde_json::json!({"unsupported":true}),
            )
            .unwrap();
        } else {
            fs::set_permissions(
                candidate.parent().unwrap(),
                fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        assert!(state.sweep_metadata_probes_checked().is_err());
        assert!(candidate.exists());
    }
}
