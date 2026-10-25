use super::*;
use crate::state::sync_tests::with_hook;
use atomic_fs::Step;
use std::{io, os::unix::fs::PermissionsExt};
mod waiter;
fn fixture() -> (tempfile::TempDir, State) {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let state = State::new(tmp.path().join("wrapper")).unwrap();
    drop(state.initialize().unwrap());
    state::directory(&state.root.join("native-probes")).unwrap();
    (tmp, state)
}
#[test]
fn wrapper_below_public_parent_is_nonrecursive_and_reports_removed_sync_failure() {
    for (lock, step) in [
        (true, Step::BeforeRemove),
        (true, Step::AfterRemove),
        (false, Step::BeforeRemove),
        (false, Step::AfterRemove),
        (false, Step::BeforeDirectorySync),
        (false, Step::AfterDirectorySync),
    ] {
        let (_tmp, state) = fixture();
        let identity = state.metadata_root_identity().unwrap();
        let path = if lock {
            state.root.join("native-probes.lock")
        } else {
            state.root.clone()
        };
        let result = with_hook(
            move |at, point| {
                if at == path && point == step {
                    Err(io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
            || state.remove_empty_metadata_root(&identity),
        );
        assert_eq!(result.warning.unwrap().io_raw_os_error(), Some(libc::EIO));
        assert_eq!(result.removed, !lock && step != Step::BeforeRemove);
        assert_eq!(state.root.exists(), !result.removed);
        if !result.removed {
            let result = state.remove_empty_metadata_root(&identity);
            assert!(result.removed && result.warning.is_none());
        }
    }
}
#[test]
fn unsafe_unknown_or_replaced_wrappers_remain_untouched() {
    for defect in ["unknown", "permit", "mode", "replacement", "symlink"] {
        let (tmp, state) = fixture();
        let identity = state.metadata_root_identity().unwrap();
        match defect {
            "unknown" => {
                state::private_file(&state.root.join("unknown"), true).unwrap();
            }
            "permit" => {
                state::private_file(&state.root.join("native-probes/unknown.prune.json"), true)
                    .unwrap();
            }
            "mode" => fs::set_permissions(&state.root, fs::Permissions::from_mode(0o755)).unwrap(),
            _ => {
                fs::rename(&state.root, tmp.path().join("saved")).unwrap();
                if defect == "symlink" {
                    std::os::unix::fs::symlink(tmp.path().join("saved"), &state.root).unwrap();
                } else {
                    state::directory(&state.root).unwrap();
                }
            }
        }
        let cleanup = state.remove_empty_metadata_root(&identity);
        assert!(!cleanup.removed && cleanup.warning.is_some(), "{defect}");
        assert!(state.root.exists());
    }
}
#[test]
fn new_lock_or_replaced_wrapper_after_unlink_is_never_deleted() {
    for replacement in [false, true] {
        let (tmp, state) = fixture();
        let identity = state.metadata_root_identity().unwrap();
        let root = state.root.clone();
        let saved = tmp.path().join("saved");
        let result = with_hook(
            move |path, step| {
                if path == root.join("native-probes.lock") && step == Step::AfterRemove {
                    if replacement {
                        fs::rename(&root, &saved)?;
                        state::directory(&root).unwrap();
                    } else {
                        state::private_file(path, true).unwrap();
                    }
                }
                Ok(())
            },
            || state.remove_empty_metadata_root(&identity),
        );
        assert!(!result.removed && result.warning.is_some());
        assert!(state.root.exists());
        if replacement {
            assert!(tmp.path().join("saved").exists());
        } else {
            assert!(state.root.join("native-probes.lock").exists());
        }
    }
}
