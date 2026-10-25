//! Indexing regressions.
use super::*;

#[test]
fn shared_status_never_backfills_identity_and_changed_cached_pins_are_rejected() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let lease = state.runtime_lease(&binding).unwrap();
    state
        .check_account(&binding, Some("observed-account"))
        .unwrap();
    assert!(
        state.generations("main").unwrap()[0]
            .account_identity
            .is_none()
    );
    lease.verify(&state, &binding).unwrap();
    // Model an unexpected metadata update; normal auth changes are exclusive
    // and cannot run beside this live credential lease.
    state
        .record_account(&binding, Some("new-pin".into()))
        .unwrap();
    assert!(lease.verify(&state, &binding).is_err());
    assert!(
        state
            .check_account(&binding, Some("other-account"))
            .is_err()
    );
}

#[test]
fn profile_index_never_synthesizes_missing_generation_or_lock_identity() {
    enum Defect {
        Unexpected,
        Generations,
        CredentialLock,
        Directory,
    }

    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let path = state.root.join("index.json");
    let original: serde_json::Value = read_json(&path).unwrap();
    for defect in [
        Defect::Unexpected,
        Defect::Generations,
        Defect::CredentialLock,
        Defect::Directory,
    ] {
        let field = match defect {
            Defect::Unexpected => "unexpected",
            Defect::Generations => "generations",
            Defect::CredentialLock => "credential_lock",
            Defect::Directory => "directory",
        };
        let mut damaged = original.clone();
        match defect {
            Defect::Unexpected => damaged[field] = serde_json::json!(1),
            Defect::Generations => {
                damaged.as_object_mut().unwrap().remove(field);
            }
            Defect::Directory => {
                damaged["profiles"]["main"]
                    .as_object_mut()
                    .unwrap()
                    .remove(field);
            }
            Defect::CredentialLock => {
                damaged["generations"][binding.id.to_string()][field] = serde_json::Value::Null;
            }
        }
        write_json(&path, &damaged).unwrap();
        let before = fs::read(&path).unwrap();
        assert!(state.lookup("main", &binding.profile).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}
