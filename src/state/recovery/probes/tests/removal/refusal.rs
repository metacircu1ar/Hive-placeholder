use super::*;
use std::os::unix::fs::PermissionsExt;
#[test]
fn permits_never_authorize_unknown_replaced_or_unsafe_trees() {
    let _fork = crate::os::fork_guard().unwrap();
    for defect in [
        "both", "tree", "parent", "id", "lock", "unknown", "unsafe", "symlink", "aside",
    ] {
        let (_tmp, state, dir, id) = fixture();
        fail(
            &state,
            &dir,
            id,
            Fault {
                permit: false,
                step: Step::BeforePublish,
                occurrence: 1,
            },
        );
        let permit = dir.with_file_name(format!("{id}.prune.json"));
        let aside = dir.with_file_name(format!("{id}.pruning"));
        match defect {
            "both" => state::directory(&aside).unwrap(),
            "tree" => {
                fs::rename(&dir, state.root.join("saved")).unwrap();
                state::directory(&dir).unwrap();
            }
            "lock" => {
                // Keep the old inode allocated: unlink/recreate can reuse it on
                // Linux and would not exercise the intended identity mismatch.
                fs::rename(dir.join("auth.lock"), state.root.join("saved-lock")).unwrap();
                state::private_file(&dir.join("auth.lock"), true).unwrap();
            }
            "unsafe" => fs::set_permissions(&permit, fs::Permissions::from_mode(0o644)).unwrap(),
            "symlink" => {
                fs::rename(&permit, state.root.join("permit")).unwrap();
                std::os::unix::fs::symlink(state.root.join("permit"), &permit).unwrap();
            }
            "aside" => {
                fs::rename(&dir, &aside).unwrap();
                fs::remove_file(&permit).unwrap();
            }
            _ => {
                let mut value: serde_json::Value = read_json(&permit).unwrap();
                match defect {
                    "parent" => value["parent"]["inode"] = 0.into(),
                    "id" => value["id"] = Uuid::new_v4().to_string().into(),
                    _ => value["unexpected"] = true.into(),
                }
                write_json(&permit, &value).unwrap();
            }
        }
        assert!(finish_probe(&state, &dir, id).is_err(), "{defect}");
        assert!(dir.exists() || aside.exists(), "{defect}");
        // Refusal of one namespace must not prevent allocating a new isolated probe.
        state.metadata_probe().unwrap().finish().unwrap();
        assert!(dir.exists() || aside.exists(), "{defect}");
    }
}
#[test]
fn allocation_refuses_original_permit_or_aside_and_sweep_ignores_unknown_names() {
    let (_tmp, state, dir, id) = fixture();
    assert!(allocate(&dir).is_err());
    let unknown = dir.with_file_name("not-a-probe.pruning");
    state::directory(&unknown).unwrap();
    let _fork = crate::os::fork_guard().unwrap();
    fail(
        &state,
        &dir,
        id,
        Fault {
            permit: false,
            step: Step::AfterPublish,
            occurrence: 1,
        },
    );
    assert!(allocate(&dir).is_err());
    let aside = dir.with_file_name(format!("{id}.pruning"));
    fs::remove_dir_all(aside).unwrap();
    assert!(
        allocate(&dir).is_err(),
        "permit reserves an absent original"
    );
    state.sweep_metadata_probes_checked().unwrap();
    assert!(unknown.exists());
    removed(&dir);
    allocate(&dir).unwrap();
}
