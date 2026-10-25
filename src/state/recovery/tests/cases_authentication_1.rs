//! Authentication regressions.
use super::*;

#[test]
fn shared_probes_do_not_create_writers_and_weak_lease_release_allows_login() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let lease = Arc::new(state.runtime_lease(&binding).unwrap());
    let weak = Arc::downgrade(&lease);
    let mut probe = state
        .operation(&binding, crate::state::OperationKind::Status)
        .unwrap();
    let child = probe.prepare_child(true, false).unwrap();
    child.started(std::process::id()).unwrap();
    child.finish(true).unwrap();
    drop(child);
    probe.finish().unwrap();
    assert!(matches!(
        state.operation(&binding, crate::state::OperationKind::Login),
        Err(Error::ProfileBusy { .. })
    ));
    drop(lease);
    assert!(weak.upgrade().is_none());
    state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap()
        .finish()
        .unwrap();
}

#[test]
fn replaced_credential_lock_is_not_a_runtime_or_mutation_bypass() {
    let (tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let lease = state.runtime_lease(&binding).unwrap();
    let path = binding.directory.join("auth.lock");
    fs::rename(&path, tmp.path().join("retained-auth.lock")).unwrap();
    assert!(lease.verify(&state, &binding).is_err());
    assert!(state.runtime_lease(&binding).is_err());
    assert!(
        state
            .operation(&binding, crate::state::OperationKind::Login)
            .is_err()
    );
    drop(lease);
    let repair = state.begin_repair("main", &binding.profile).unwrap();
    state.runtime_lease(&repair.binding).unwrap();
}

#[test]
fn corrupt_runtime_history_is_reported_and_does_not_invalidate_credentials() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    state.runtime_lease(&binding).unwrap();
    let base = binding.directory.join("executions");
    directory(&base).unwrap();
    let path = base.join(format!("{}.json", Uuid::new_v4()));
    write_json(&path, &serde_json::json!({"unreadable":"record"})).unwrap();
    state.runtime_lease(&binding).unwrap();
    let report = crate::runtime::api::value(state.recovery_report(&binding).unwrap());
    assert_eq!(report["runtime_usable"], true);
    assert!(report["history_error"].is_object());
    assert!(
        state
            .operation(&binding, crate::state::OperationKind::Login)
            .is_err()
    );
    assert!(state.recovery_reservations().unwrap()[0].unknown_capacity);
}

#[test]
fn pending_writer_intent_survives_caller_death_and_reconciles_without_authorization() {
    let (_tmp, state, binding) = fixture();
    let status = crate::os::spawn(
        crate::test_support::rerun_isolated("state::recovery::tests::launch_crash_fixture", true)
            .env("HIVE_RECOVERY_TEST_ROOT", &state.root)
            .env("HIVE_RECOVERY_TEST_PHASE", "writer_pending")
            .stdout(std::process::Stdio::null()),
    )
    .and_then(|mut child| child.wait())
    .unwrap();
    assert_eq!(status.code(), Some(79));
    assert!(state.runtime_lease(&binding).is_err());
    let mut operation = state
        .operation(&binding, crate::state::OperationKind::Recover)
        .unwrap();
    complete_reconciliation_probe(&mut operation);
    operation.confirm_missing().unwrap();
    operation.finish().unwrap();
    state.runtime_lease(&binding).unwrap();
}

#[test]
fn live_verification_probe_cannot_clear_pending_credentials() {
    use std::os::unix::process::CommandExt;
    let (_tmp, state, binding) = fixture();
    let mut op = state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap();
    let writer = op.prepare_child(true, true).unwrap();
    writer.started(std::process::id()).unwrap();
    writer.finish(true).unwrap();
    let probe = op.prepare_child(true, false).unwrap();
    let mut child = crate::test_support::ChildGuard(
        crate::os::spawn(
            std::process::Command::new("/bin/sleep")
                .arg("10")
                .process_group(0),
        )
        .unwrap(),
    );
    probe.started(child.id()).unwrap();
    assert!(op.confirm_missing().is_err());
    child.kill().unwrap();
    child.wait().unwrap();
    probe.finish(false).unwrap();
    op.confirm_missing().unwrap();
    assert!(
        state
            .pending_credential_attempt(binding.id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn standalone_metadata_probes_never_grant_credential_writer_authority() {
    let (_tmp, state, binding) = fixture();
    let _busy = state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap();
    let mut metadata = state.metadata_probe().unwrap();
    assert!(metadata.prepare_child(true, true).is_err());
    assert!(metadata.confirm_missing().is_err());
    assert!(metadata.confirm_account(None).is_err());
    complete_reconciliation_probe(&mut metadata);
    metadata.verify_probe_ownership().unwrap();
    metadata.finish().unwrap();
}

#[test]
fn metadata_probes_reject_symlinked_state_without_touching_destination() {
    let tmp = tempfile::tempdir().unwrap();
    let destination = tmp.path().join("destination");
    directory(&destination).unwrap();
    let root = tmp.path().join("state");
    std::os::unix::fs::symlink(&destination, &root).unwrap();
    let state = State::new(root).unwrap();
    assert!(state.metadata_probe().is_err());
    assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
}
