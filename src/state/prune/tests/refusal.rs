use super::{recovery::*, *};
use crate::state::{self, sync_tests::with_hook};
use atomic_fs::Step;
use std::{fs, io, os::unix::fs::PermissionsExt};

fn paused() -> Fixture {
    let fixture = Fixture::new();
    fail(
        &fixture,
        Fault {
            leaf: "provider",
            step: Step::AfterPublish,
            occurrence: 1,
        },
    );
    fixture
}
#[test]
fn pruning_rejects_unknown_asides_missing_locks_and_malformed_intents() {
    let _fork = crate::os::fork_guard().unwrap();
    for defect in [
        "both",
        "replacement",
        "symlink",
        "public",
        "lock_missing",
        "lock_replaced",
        "intent",
        "unknown",
    ] {
        let fixture = paused();
        let dir = &fixture.binding.directory;
        let aside = dir.join("provider.pruning");
        let saved = fixture.state.root.join("saved");
        match defect {
            "both" => state::directory(&dir.join("provider")).unwrap(),
            "replacement" | "symlink" | "public" => {
                fs::rename(&aside, &saved).unwrap();
                if defect == "symlink" {
                    std::os::unix::fs::symlink(&saved, &aside).unwrap();
                } else {
                    state::directory(&aside).unwrap();
                    state::write_json(&aside.join("sentinel"), &serde_json::json!(true)).unwrap();
                    if defect == "public" {
                        fs::set_permissions(&aside, fs::Permissions::from_mode(0o755)).unwrap();
                    }
                }
            }
            "lock_missing" | "lock_replaced" => {
                fs::rename(dir.join("auth.lock"), &saved).unwrap();
                if defect == "lock_replaced" {
                    drop(Lock::acquire(&dir.join("auth.lock")).unwrap());
                }
            }
            "intent" => {
                state::write_json(&dir.join("prune-intent.json"), &serde_json::json!({})).unwrap();
            }
            "unknown" => state::directory(&dir.join("unknown")).unwrap(),
            _ => unreachable!(),
        }
        assert!(fixture.prune().is_err(), "{defect}");
        assert!(!dir.join("prune-receipt.json").exists());
        assert!(dir.join("work/a").exists());
        if matches!(defect, "replacement" | "symlink" | "public") {
            assert!(saved.join("a").exists());
        }
    }
}
#[test]
fn receipt_retries_refuse_retained_asides_and_nonempty_replacements() {
    let _fork = crate::os::fork_guard().unwrap();
    for terminal in [false, true] {
        for defect in ["aside", "populated", "symlink"] {
            let fixture = Fixture::new();
            if terminal {
                fixture.prune().unwrap();
            } else {
                fail(
                    &fixture,
                    Fault {
                        leaf: "prune-receipt.json",
                        step: Step::AfterPublish,
                        occurrence: 1,
                    },
                );
            }
            let dir = &fixture.binding.directory;
            let receipt = fs::read(dir.join("prune-receipt.json")).unwrap();
            if defect == "aside" {
                state::directory(&dir.join("work.pruning")).unwrap();
            } else {
                let path = dir.join("provider");
                if path.exists() {
                    fs::remove_dir(&path).unwrap();
                }
                if defect == "symlink" {
                    std::os::unix::fs::symlink("missing", &path).unwrap();
                } else {
                    state::directory(&path).unwrap();
                    state::write_json(&path.join("sentinel"), &serde_json::json!(true)).unwrap();
                }
            }
            assert!(fixture.prune().is_err(), "{terminal}/{defect}");
            assert_eq!(fs::read(dir.join("prune-receipt.json")).unwrap(), receipt);
        }
    }
}
#[test]
fn pre_intent_is_fenced_and_existing_lock_is_mandatory_on_retry() {
    let _fork = crate::os::fork_guard().unwrap();
    let fixture = Fixture::new();
    fail(
        &fixture,
        Fault {
            leaf: "index.json",
            step: Step::AfterDirectorySync,
            occurrence: 1,
        },
    );
    fixture.fenced();
    let lock = Lock::acquire_existing(&fixture.binding.directory.join("auth.lock")).unwrap();
    assert_eq!(fixture.prune().unwrap_err().code(), "profile_busy");
    drop(lock);
    fixture.prune().unwrap();
    fixture.completed();
}
#[test]
fn reader_observes_complete_original_until_rename_and_never_partial_original() {
    let _fork = crate::os::fork_guard().unwrap();
    let fixture = Fixture::new();
    let (requests, observations) = std::sync::mpsc::channel();
    let (answers, replies) = std::sync::mpsc::channel();
    let root = fixture.binding.directory.clone();
    let reader = std::thread::spawn(move || {
        while let Ok(step) = observations.recv() {
            let path = root.join("provider");
            if step == Step::BeforePublish {
                assert!(path.join("a").exists() && path.join("b").exists());
            } else {
                assert!(!path.exists());
            }
            answers.send(()).unwrap();
        }
    });
    let error = with_hook(
        move |path, step| {
            if path.file_name().unwrap() == "provider"
                && matches!(
                    step,
                    Step::BeforePublish | Step::AfterPublish | Step::BeforeRemove
                )
            {
                requests.send(step).unwrap();
                replies.recv().unwrap();
                if step == Step::BeforeRemove {
                    fs::remove_file(path.with_file_name("provider.pruning").join("a"))?;
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
            }
            Ok(())
        },
        || fixture.prune(),
    )
    .unwrap_err();
    reader.join().unwrap();
    assert_eq!(native_cause(&error), Some(libc::EIO));
    assert!(!fixture.binding.directory.join("provider").exists());
    fixture.prune().unwrap();
    fixture.completed();
}

#[test]
fn both_tree_identities_are_checked_before_any_tree_is_removed() {
    let _fork = crate::os::fork_guard().unwrap();
    let fixture = Fixture::new();
    fail(
        &fixture,
        Fault {
            leaf: "provider",
            step: Step::BeforePublish,
            occurrence: 1,
        },
    );
    let dir = &fixture.binding.directory;
    let saved = fixture.state.root.join("retained-work");
    fs::rename(dir.join("work"), &saved).unwrap();
    state::directory(&dir.join("work")).unwrap();
    assert!(fixture.prune().is_err());
    assert!(dir.join("provider/a").exists() && dir.join("provider/b").exists());
    assert!(!dir.join("provider.pruning").exists());
    assert!(saved.join("a").exists());
    fixture.unchanged();
}

#[test]
fn intent_identity_mismatches_preserve_both_original_trees() {
    let _fork = crate::os::fork_guard().unwrap();
    for field in ["generation", "profile", "credential_lock"] {
        let fixture = Fixture::new();
        fail(
            &fixture,
            Fault {
                leaf: "provider",
                step: Step::BeforePublish,
                occurrence: 1,
            },
        );
        let path = fixture.binding.directory.join("prune-intent.json");
        let mut intent: Value = state::read_json(&path).unwrap();
        intent[field] = match field {
            "generation" => serde_json::json!(Uuid::new_v4()),
            "profile" => serde_json::json!("different"),
            _ => serde_json::json!({"dev": 0, "ino": 0}),
        };
        state::write_json(&path, &intent).unwrap();
        assert!(fixture.prune().is_err(), "{field}");
        for tree in ["provider", "work"] {
            assert!(fixture.binding.directory.join(tree).join("a").exists());
        }
        fixture.unchanged();
    }
}
