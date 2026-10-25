//! Behaviour regressions.
use super::*;

#[test]
fn unknown_namespace_layout_blocks_ownership_decisions_without_writes() {
    for name in ["writers-v2", "unrecognized-evidence"] {
        let (_tmp, state, binding) = fixture();
        let _fork = crate::os::fork_guard().unwrap();
        state.runtime_lease(&binding).unwrap();
        let mut operation = state
            .operation(&binding, crate::state::OperationKind::Login)
            .unwrap();
        let child = operation.prepare_child(true, true).unwrap();
        child.started(std::process::id()).unwrap();
        drop(child);
        drop(operation);
        let dir = binding.directory.clone();
        fs::rename(dir.join("writers"), dir.join(name)).unwrap();
        let before = snapshot_files(&state.root);
        let assert_layout = |error: Error| assert_eq!(error.code(), "unsupported_schema");
        assert_layout(state.runtime_lease(&binding).err().unwrap());
        for kind in [
            crate::state::OperationKind::Status,
            crate::state::OperationKind::Login,
            crate::state::OperationKind::Prune,
        ] {
            assert_layout(state.operation(&binding, kind).err().unwrap());
        }
        assert_layout(state.begin_repair("main", &binding.profile).err().unwrap());
        assert_layout(state.register("main", &binding.profile).err().unwrap());
        assert_layout(state.recovery_report(&binding).unwrap_err());
        assert_layout(
            state
                .check_conversation(&binding, Some(&Uuid::new_v4().to_string()))
                .unwrap_err(),
        );
        assert_layout(state.prune("main", binding.id).unwrap_err());
        let reservations = state.recovery_reservations().unwrap();
        assert_eq!(reservations.len(), 1);
        assert!(reservations[0].unknown_capacity && reservations[0].unresolved_activity);
        assert_eq!(snapshot_files(&state.root), before);
    }
}

#[test]
fn namespace_layout_rejects_wrong_types_and_accepts_current_staging_files() {
    let (_tmp, state, binding) = fixture();
    let dir = binding.directory.clone();
    let temporary = atomic_fs::PreparedCreate::new(
        dir.join("fixture.json"),
        hive_setup_support::names::PUBLICATION_OPTIONS,
    )
    .unwrap();
    layout::profile(&dir).unwrap();
    temporary.cancel().unwrap();
    for name in ["writers", "executions", "execution-archive"] {
        crate::state::write_json(&dir.join(name), &serde_json::json!({})).unwrap();
        assert_eq!(layout::profile(&dir).unwrap_err().code(), "unsafe_state");
        fs::remove_file(dir.join(name)).unwrap();
        directory(&dir.join(name)).unwrap();
    }
    state.runtime_lease(&binding).unwrap();
}

#[test]
fn concurrent_repair_cas_and_cancel_preserve_every_namespace() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let a = state.begin_repair("main", &binding.profile).unwrap();
    let b = state.begin_repair("main", &binding.profile).unwrap();
    assert!(state.finish_repair(&a, true, None).unwrap());
    assert!(!state.finish_repair(&b, true, None).unwrap());
    let c = state.begin_repair("main", &binding.profile).unwrap();
    assert!(!state.finish_repair(&c, false, None).unwrap());
    assert_eq!(
        state.lookup("main", &binding.profile).unwrap().unwrap().id,
        a.binding.id
    );
    let generations = state.generations("main").unwrap();
    assert_eq!(generations.len(), 4);
    assert_eq!(
        generations
            .iter()
            .filter(|g| g.state == GenerationState::Retired)
            .count(),
        3
    );
    assert!(state.finish_repair(&b, true, None).is_err());
    state.prune("main", b.binding.id).unwrap();
    assert!(b.binding.directory.join("prune-receipt.json").exists());
    assert!(state.prune("main", a.binding.id).is_err());
}

#[test]
fn interrupted_atomic_write_staging_files_are_not_ownership_records() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    for name in ["", "writers", "executions"] {
        let dir = binding.directory.join(name);
        directory(&dir).unwrap();
        for name in [".hive-tmp-ABC123", ".hive-tmp-0123456789abcdef"] {
            crate::state::private_file(&dir.join(name), true)
                .unwrap()
                .write_all(b"partial")
                .unwrap();
        }
    }
    state.runtime_lease(&binding).unwrap();
    state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap()
        .finish()
        .unwrap();
    assert!(
        crate::runtime::api::value(state.recovery_report(&binding).unwrap())["history_error"]
            .is_null()
    );
    let unknown = binding.directory.join("writers/.unknown");
    write_json(&unknown, &serde_json::json!({})).unwrap();
    assert!(state.runtime_lease(&binding).is_err());
}

#[test]
fn an_unrelated_failed_writer_cannot_clear_an_existing_pending_nonce() {
    let (_tmp, state, binding) = fixture();
    let mut op = state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap();
    let first = op.prepare_child(true, true).unwrap();
    first.started(std::process::id()).unwrap();
    first.finish(true).unwrap();
    let pending = state.pending_credential_attempt(binding.id).unwrap();
    let second = op.prepare_child(true, true).unwrap();
    assert!(second.started(std::process::id()).is_err());
    assert!(second.not_started().is_err());
    assert_eq!(
        state.pending_credential_attempt(binding.id).unwrap(),
        pending
    );
    complete_reconciliation_probe(&mut op);
    op.confirm_missing().unwrap();
}

#[test]
fn unrelated_archived_records_cannot_block_exact_writer_reconciliation() {
    let (_tmp, state, binding) = fixture();
    let mut op = state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap();
    let writer = op.prepare_child(true, true).unwrap();
    writer.started(std::process::id()).unwrap();
    writer.finish(true).unwrap();
    let path = binding
        .directory
        .join("execution-archive")
        .join(format!("{}.json", Uuid::new_v4()));
    write_json(
        &path,
        &serde_json::json!({"damaged":"unrelated provider history"}),
    )
    .unwrap();
    complete_reconciliation_probe(&mut op);
    op.confirm_missing().unwrap();
}

#[test]
fn writer_lookup_rejects_symlinked_manifest_and_resolution_directories() {
    for base in ["execution-archive", "credential-resolutions"] {
        let (tmp, state, binding) = fixture();
        let mut op = state
            .operation(&binding, crate::state::OperationKind::Login)
            .unwrap();
        let writer = op.prepare_child(true, true).unwrap();
        writer.started(std::process::id()).unwrap();
        writer.finish(true).unwrap();
        drop(writer);
        drop(op);
        let dir = binding.directory.clone();
        let original = dir.join(base);
        if !original.exists() {
            directory(&original).unwrap();
        }
        let mut recover = state
            .operation(&binding, crate::state::OperationKind::Recover)
            .unwrap();
        complete_reconciliation_probe(&mut recover);
        let moved = tmp.path().join("moved");
        fs::rename(&original, &moved).unwrap();
        std::os::unix::fs::symlink(moved, original).unwrap();
        assert!(recover.confirm_missing().is_err());
        assert!(recover.has_pending().unwrap());
        drop(recover);
        assert_eq!(
            state
                .operation(&binding, crate::state::OperationKind::Recover)
                .err()
                .unwrap()
                .code(),
            "unsafe_state"
        );
    }
}

#[test]
fn never_ready_registration_rejects_repair_and_reconcile_without_creating_locks() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let pending = crate::state::test_support::edit_index(&state, |index| {
        index.profiles.get_mut("main").unwrap().ready = false;
        let generation = index.generations.get_mut(&binding.id).unwrap();
        generation.binding.ready = false;
        generation.credential_lock = None;
        generation.binding.clone()
    });
    fs::remove_file(binding.directory.join("auth.lock")).unwrap();
    assert_eq!(
        state
            .begin_repair("main", &binding.profile)
            .err()
            .unwrap()
            .code(),
        "registration_incomplete"
    );
    assert_eq!(
        ledger::reconcile(&state, &pending).unwrap_err().code(),
        "registration_incomplete"
    );
    assert_eq!(
        state.recovery_report(&pending).unwrap_err().code(),
        "registration_incomplete"
    );
    assert!(!binding.directory.join("auth.lock").exists());
}
