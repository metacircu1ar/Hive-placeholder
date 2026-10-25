//! Recovery regressions.
use super::*;

#[test]
fn runtime_failure_fences_only_its_conversation_and_preserves_evidence() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let lease = Arc::new(state.runtime_lease(&binding).unwrap());
    let id = Uuid::new_v4().to_string();
    let mut evidence = lease.begin_provider(&id).unwrap();
    evidence.started(std::process::id()).unwrap();
    evidence.finish(false, false).unwrap();
    assert!(state.check_conversation(&binding, Some(&id)).is_err());
    drop(evidence);
    drop(lease);
    state.runtime_lease(&binding).unwrap();
    state
        .operation(&binding, crate::state::OperationKind::Status)
        .unwrap()
        .finish()
        .unwrap();
    assert_eq!(
        state
            .operation(&binding, crate::state::OperationKind::Login)
            .err()
            .unwrap()
            .code(),
        "recovery_required"
    );
    let report = crate::runtime::api::value(state.recovery_report(&binding).unwrap());
    assert_eq!(report["runtime_usable"], true);
    assert_eq!(report["executions"][0]["quiescent"], false);
    assert!(
        binding
            .directory
            .join(format!("executions/{id}.json"))
            .exists()
    );
}

#[test]
fn missing_execution_receipt_cannot_establish_clean_release() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let id = Uuid::new_v4().to_string();
    assert!(state.check_conversation(&binding, Some(&id)).is_err());
    let lease = Arc::new(state.runtime_lease(&binding).unwrap());
    let mut evidence = lease.begin_provider(&id).unwrap();
    evidence.finish(true, true).unwrap();
    state.check_conversation(&binding, Some(&id)).unwrap();
    let archive = binding
        .directory
        .join(format!("execution-archive/{id}.json"));
    assert!(archive.exists());
}

#[test]
fn unfinished_writer_requires_new_generation_without_changing_old_binding() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let mut operation = state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap();
    operation
        .prepare_child(true, true)
        .unwrap()
        .started(std::process::id())
        .unwrap();
    drop(operation);
    assert_eq!(
        state.runtime_lease(&binding).err().unwrap().code(),
        "recovery_required"
    );
    let repair = state.begin_repair("main", &binding.profile).unwrap();
    assert_ne!(repair.binding.directory, binding.directory);
    state.runtime_lease(&repair.binding).unwrap();
    assert!(
        state
            .finish_repair(&repair, true, Some("account-a".into()))
            .unwrap()
    );
    assert_eq!(
        state.lookup("main", &binding.profile).unwrap().unwrap().id,
        repair.binding.id
    );
    assert_eq!(
        state
            .generation("main", &binding.profile, binding.id)
            .unwrap()
            .id,
        binding.id
    );
    assert!(state.runtime_lease(&binding).is_err());
    let fresh = state.runtime_lease(&repair.binding).unwrap();
    assert!(fresh.verify_account(Some("account-b")).is_err());
    fresh.verify_account(None).unwrap(); // Lack of native identity is not invented proof.
    assert!(state.runtime_lease(&binding).is_err());
}

#[test]
fn corrupt_writer_history_blocks_only_old_generation() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let dir = binding.directory.join("writers");
    directory(&dir).unwrap();
    write_json(&dir.join("bad.json"), &serde_json::json!({})).unwrap();
    assert!(state.runtime_lease(&binding).is_err());
    let repair = state.begin_repair("main", &binding.profile).unwrap();
    state.runtime_lease(&repair.binding).unwrap();
}

#[test]
fn stale_receipt_cannot_overwrite_another_execution_revision() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let lease = Arc::new(state.runtime_lease(&binding).unwrap());
    let id = Uuid::new_v4().to_string();
    let mut receipt = lease.begin_provider(&id).unwrap();
    let path = binding.directory.join(format!("executions/{id}.json"));
    let mut value: serde_json::Value = read_json(&path).unwrap();
    value["run"] = Uuid::new_v4().to_string().into();
    write_json(&path, &value).unwrap();
    assert!(receipt.finish(true, true).is_err());
    let saved: serde_json::Value = read_json(&path).unwrap();
    assert_eq!(saved, value);
}

#[test]
fn pid_birth_identity_rejects_malformed_identity_and_does_not_signal_reused_pid() {
    let current = ProcessIdentity::current().unwrap();
    assert!(!current.absent().unwrap());
    let mut prior = current;
    prior.birth = "malformed".into();
    assert!(prior.absent().is_err());
    #[cfg(target_os = "macos")]
    {
        prior.birth = "macos:1:0".into();
    }
    #[cfg(target_os = "linux")]
    {
        prior.birth = "linux:1".into();
    }
    assert!(prior.absent().unwrap());
    assert!(ProcessIdentity::read(0).is_err());
    assert!(crate::state::same_boot("invalid").is_err());
    assert!(crate::state::same_boot(&crate::state::boot_identity().unwrap()).unwrap());
}

#[test]
fn pruning_without_intent_cannot_repair_missing_content_directories() {
    let (_tmp, state, binding) = fixture();
    // This test has no child processes. Keep unrelated test forks from briefly
    // retaining a just-released credential OFD before their close-on-exec runs.
    let _fork = crate::os::fork_guard().unwrap();
    let repair = state.begin_repair("main", &binding.profile).unwrap();
    state.finish_repair(&repair, true, None).unwrap();
    crate::state::test_support::edit_index(&state, |index| {
        index.generations.get_mut(&binding.id).unwrap().state = GenerationState::Pruning;
    });
    fs::remove_dir_all(binding.directory.join("provider")).unwrap();
    // Maintenance admission creates this stable lock before checking credential trees.
    drop(crate::runtime::maintenance_lock(&state.root).unwrap());
    let before = snapshot_files(&state.root);
    assert!(state.prune("main", binding.id).is_err());
    assert_eq!(snapshot_files(&state.root), before);
    assert!(!binding.directory.join("provider").exists());
    assert!(!binding.directory.join("prune-receipt.json").exists());
}

#[test]
fn abandoned_candidate_is_prunable_but_active_repair_is_not() {
    for ready in [true, false] {
        let (_tmp, state, binding) = fixture();
        let repair = state.begin_repair("main", &binding.profile).unwrap();
        assert_eq!(
            state.prune("main", repair.binding.id).unwrap_err().code(),
            "profile_busy"
        );
        let mut child = crate::test_support::ChildGuard(
            crate::os::spawn(std::process::Command::new("/bin/sleep").arg("10")).unwrap(),
        );
        let owner = ProcessIdentity::read(child.id()).unwrap().unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        crate::state::test_support::edit_index(&state, |index| {
            let generation = index.generations.get_mut(&repair.binding.id).unwrap();
            generation.repair_owner = Some(owner);
            if !ready {
                generation.binding.ready = false;
                generation.credential_lock = None;
                let dir = repair.binding.directory.clone();
                fs::remove_file(dir.join("auth.lock")).unwrap();
                write_json(&dir.join("identity.json"), &generation.binding).unwrap();
            }
        });
        state.prune("main", repair.binding.id).unwrap();
        assert_eq!(
            state.lookup("main", &binding.profile).unwrap().unwrap().id,
            binding.id
        );
    }
}

#[test]
fn cutover_rejects_a_replaced_candidate_lock_before_retiring_active_generation() {
    let (tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    let repair = state.begin_repair("main", &binding.profile).unwrap();
    state.runtime_lease(&repair.binding).unwrap();
    let lock = repair.binding.directory.join("auth.lock");
    fs::rename(&lock, tmp.path().join("retained-auth.lock")).unwrap();
    assert!(state.finish_repair(&repair, true, None).is_err());
    assert_eq!(
        state.lookup("main", &binding.profile).unwrap().unwrap().id,
        binding.id
    );
}

#[test]
fn dead_probe_allows_in_place_login_and_prune_without_conversation_proof() {
    use std::os::unix::process::CommandExt;
    let (_tmp, state, binding) = fixture();
    let mut operation = state
        .operation(&binding, crate::state::OperationKind::Status)
        .unwrap();
    let receipt = operation.prepare_child(true, false).unwrap();
    let mut child = crate::test_support::ChildGuard(
        crate::os::spawn(
            std::process::Command::new("/bin/sleep")
                .arg("10")
                .process_group(0),
        )
        .unwrap(),
    );
    receipt.started(child.id()).unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    // The child is reaped; exclude unrelated test forks while dropping the old
    // credential descriptor and immediately asserting exclusive reacquisition.
    let _fork = crate::os::fork_guard().unwrap();
    receipt.finish(false).unwrap();
    drop(receipt);
    operation.finish().unwrap();
    state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap()
        .finish()
        .unwrap();
    let replacement = state.begin_repair("main", &binding.profile).unwrap();
    state.finish_repair(&replacement, true, None).unwrap();
    state.prune("main", binding.id).unwrap();
}

#[test]
fn completed_writer_stays_fenced_until_account_verification_and_failed_repair_keeps_evidence() {
    let (_tmp, state, binding) = fixture();
    let _fork = crate::os::fork_guard().unwrap();
    state
        .record_account(&binding, Some("before".into()))
        .unwrap();
    let mut op = state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap();
    let receipt = op.prepare_child(true, true).unwrap();
    receipt.started(std::process::id()).unwrap();
    receipt.finish(true).unwrap();
    drop(receipt);
    op.finish().unwrap();
    assert!(state.runtime_lease(&binding).is_err());
    let g = state.generations("main").unwrap().remove(0);
    assert!(g.possibly_authenticated && g.credential_mutation.is_some());
    assert!(
        state
            .finish_credential_attempt(
                binding.id,
                g.credential_mutation.unwrap(),
                g.credential_writer.unwrap(),
                Some("after".into()),
                false
            )
            .is_err()
    );
    let repair = state.begin_repair("main", &binding.profile).unwrap();
    state
        .record_account(&repair.binding, Some("new".into()))
        .unwrap();
    state.finish_repair(&repair, false, None).unwrap();
    let retired = state
        .generations("main")
        .unwrap()
        .into_iter()
        .find(|g| g.binding.id == repair.binding.id)
        .unwrap();
    assert!(retired.authenticated && retired.possibly_authenticated);
    assert_eq!(retired.account_identity.as_deref(), Some("new"));
}

#[test]
fn reconciliation_and_prune_refuse_missing_duplicate_or_replaced_writer_manifest() {
    #[derive(Clone, Copy, Debug)]
    enum Defect {
        Missing,
        Duplicate,
        Nonce,
    }
    for invalid in [Defect::Missing, Defect::Duplicate, Defect::Nonce] {
        let (_tmp, state, binding) = fixture();
        let _fork = crate::os::fork_guard().unwrap();
        let mut operation = state
            .operation(&binding, crate::state::OperationKind::Login)
            .unwrap();
        let child = operation.prepare_child(true, true).unwrap();
        child.started(std::process::id()).unwrap();
        child.finish(true).unwrap();
        drop(child);
        drop(operation);
        let attempt = state
            .pending_credential_attempt(binding.id)
            .unwrap()
            .unwrap();
        let dir = binding.directory.clone();
        let path = dir.join(format!("execution-archive/{}.json", attempt.writer));
        let record: ledger::Record = read_json(&path).unwrap();
        match invalid {
            Defect::Missing => fs::remove_file(path).unwrap(),
            Defect::Duplicate => {
                directory(&dir.join("writers")).unwrap();
                write_json(
                    &dir.join(format!("writers/{}.json", attempt.writer)),
                    &record,
                )
                .unwrap();
            }
            Defect::Nonce => {
                let mut changed = record;
                changed.run = Uuid::new_v4();
                assert_ne!(changed.run, attempt.run);
                write_json(&path, &changed).unwrap();
            }
        }
        assert!(
            state
                .operation(&binding, crate::state::OperationKind::Recover)
                .is_err()
        );
        assert!(
            state
                .operation(&binding, crate::state::OperationKind::Prune)
                .is_err()
        );
    }
}

#[test]
fn retired_account_fence_does_not_prevent_proven_local_credential_pruning() {
    let (_tmp, state, binding) = fixture();
    state.record_account(&binding, Some("old".into())).unwrap();
    let mut op = state
        .operation(&binding, crate::state::OperationKind::Login)
        .unwrap();
    let writer = op.prepare_child(true, true).unwrap();
    writer.started(std::process::id()).unwrap();
    writer.finish(true).unwrap();
    complete_reconciliation_probe(&mut op);
    assert!(op.confirm_account(Some("changed".into())).is_err());
    drop(writer);
    drop(op);
    let evidence = binding.directory.join("execution-archive");
    let before: Vec<_> = fs::read_dir(&evidence)
        .unwrap()
        .map(|p| fs::read(p.unwrap().path()).unwrap())
        .collect();
    let repair = state.begin_repair("main", &binding.profile).unwrap();
    state
        .finish_repair(&repair, true, Some("new".into()))
        .unwrap();
    let report = state.prune("main", binding.id).unwrap();
    assert_eq!(report["outcome"], "pruned");
    assert!(!before.is_empty());
    let after: Vec<_> = fs::read_dir(evidence)
        .unwrap()
        .map(|p| fs::read(p.unwrap().path()).unwrap())
        .collect();
    assert_eq!(before, after);
}
