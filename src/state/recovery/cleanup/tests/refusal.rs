use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn refused_credential_admission_never_cleans_preparations() {
    let _fork = crate::os::fork_guard().unwrap();
    for defect in ["busy", "index", "pending", "fence", "execution", "lock"] {
        let fixture = Fixture::new();
        let retained = if defect == "busy" {
            Some(fixture.state.runtime_lease(&fixture.binding).unwrap())
        } else {
            None
        };
        if defect == "pending" || defect == "execution" {
            let mut operation = fixture.begin(OperationKind::Login).unwrap();
            let receipt = operation.prepare_child(false, defect == "pending").unwrap();
            receipt.started(std::process::id()).unwrap();
        }
        let candidate = fixture.candidate("");
        match defect {
            "index" => state::write_json(
                &fixture.state.root.join("index.json"),
                &serde_json::json!({"unsupported":true}),
            )
            .unwrap(),
            "fence" => state::test_support::edit_index(&fixture.state, |index| {
                index
                    .generation_mut(fixture.binding.id)
                    .unwrap()
                    .credential_fence = true;
            }),
            "lock" => {
                let lock = fixture.binding.directory.join("auth.lock");
                fs::rename(&lock, fixture.state.root.join("saved-lock")).unwrap();
                state::private_file(&lock, true).unwrap();
            }
            _ => {}
        }
        assert!(fixture.begin(OperationKind::Login).is_err(), "{defect}");
        assert!(candidate.exists(), "{defect}");
        drop(retained);
    }
}

#[test]
fn unsafe_candidates_remain_and_their_targets_are_not_followed() {
    let _fork = crate::os::fork_guard().unwrap();
    for kind in ["public", "directory", "symlink", "hardlink"] {
        let fixture = Fixture::new();
        let directory = fixture.directory("recovery-receipts");
        let saved = directory.join("sentinel.json");
        state::write_json(&saved, &true).unwrap();
        let candidate = directory.join(".hive-tmp-state-ABC123");
        match kind {
            "public" => {
                fixture.candidate("recovery-receipts");
                fs::set_permissions(&candidate, fs::Permissions::from_mode(0o644)).unwrap();
            }
            "directory" => state::directory(&candidate).unwrap(),
            "symlink" => std::os::unix::fs::symlink(&saved, &candidate).unwrap(),
            "hardlink" => fs::hard_link(&saved, &candidate).unwrap(),
            _ => unreachable!(),
        }
        assert_eq!(
            fixture.begin(OperationKind::Login).err().unwrap().code(),
            "unsafe_state"
        );
        assert!(candidate.symlink_metadata().is_ok());
        assert_eq!(fs::read(&saved).unwrap(), b"true\n");
    }
}

#[test]
fn every_original_lock_is_rechecked_before_deletion_and_sync() {
    let _fork = crate::os::fork_guard().unwrap();
    for name in ["auth.lock", "index.lock", "evidence.lock"] {
        for at in [Step::BeforeRemove, Step::BeforeDirectorySync] {
            let fixture = Fixture::new();
            let leaf = if name == "evidence.lock" {
                "recovery-receipts"
            } else {
                ""
            };
            let candidate = fixture.candidate(leaf);
            let target = if at == Step::BeforeRemove {
                candidate.clone()
            } else {
                fixture.directory(leaf)
            };
            let lock = if name == "index.lock" {
                fixture.state.root.join(name)
            } else {
                fixture.binding.directory.join(name)
            };
            let saved = fixture.state.root.join("saved-lock");
            let error = with_hook(
                move |path, step| {
                    if path == target && step == at {
                        fs::rename(&lock, &saved)?;
                        state::private_file(&lock, true).unwrap();
                    }
                    Ok(())
                },
                || fixture.begin(OperationKind::Login),
            )
            .err()
            .unwrap();
            assert_eq!(error.code(), "unsafe_state", "{name}/{at:?}");
            assert_eq!(candidate.exists(), at == Step::BeforeRemove);
        }
    }
}
