use super::*;
use crate::{
    config::{Method, Profile, Provider},
    state::{OperationKind, sync_tests::with_hook},
};
use atomic_fs::Step;
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use uuid::Uuid;
mod concurrency;
mod death;
mod refusal;

struct Fixture {
    _temporary: tempfile::TempDir,
    state: State,
    binding: Binding,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let state = State::new(temporary.path().join("state")).unwrap();
        let binding = state
            .register(
                "main",
                &Profile {
                    provider: Provider::Codex,
                    method: Method::Subscription,
                    api_key_env: None,
                    api_key_file: None,
                },
            )
            .unwrap();
        Self {
            _temporary: temporary,
            state,
            binding,
        }
    }
    fn begin(&self, kind: OperationKind) -> Result<super::super::operations::Operation> {
        self.state.operation(&self.binding, kind)
    }
    fn directory(&self, leaf: &str) -> PathBuf {
        let path = self.binding.directory.join(leaf);
        state::directory(&path).unwrap();
        path
    }
    fn candidate(&self, leaf: &str) -> PathBuf {
        let path = self.directory(leaf).join(".hive-tmp-state-ABC123");
        state::private_file(&path, true).unwrap();
        path
    }
    fn retire(&self) {
        let repair = self
            .state
            .begin_repair("main", &self.binding.profile)
            .unwrap();
        assert!(self.state.finish_repair(&repair, true, None).unwrap());
    }
}

#[test]
fn admitted_exclusive_operations_clean_only_owned_immediate_files() {
    let _fork = crate::os::fork_guard().unwrap();
    for kind in [
        OperationKind::Login,
        OperationKind::Recover,
        OperationKind::Prune,
    ] {
        let fixture = Fixture::new();
        // Generate actual committed ledger/archive and recovery receipts first.
        let lease = Arc::new(fixture.state.runtime_lease(&fixture.binding).unwrap());
        let mut provider = lease.begin_provider(&Uuid::new_v4().to_string()).unwrap();
        provider.finish(true, true).unwrap();
        drop(provider);
        drop(lease);
        fixture.state.recovery_report(&fixture.binding).unwrap();
        if kind == OperationKind::Prune {
            fixture.retire();
        }
        let mut retained = Vec::new();
        let mut candidates = Vec::new();
        for leaf in std::iter::once("").chain(LEAVES) {
            candidates.push(fixture.candidate(leaf));
            let generic = fixture.directory(leaf).join(".hive-tmp-ABC123");
            state::write_json(&generic, &true).unwrap();
            retained.push(generic);
        }
        for leaf in ["provider", "work", "recovery-receipts/nested"] {
            retained.push(fixture.candidate(leaf));
        }
        for leaf in ["execution-archive", "recovery-receipts"] {
            retained.extend(
                fs::read_dir(fixture.directory(leaf))
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .filter(|path| path.extension().is_some_and(|e| e == "json")),
            );
        }
        retained.push(fixture.binding.directory.join("identity.json"));
        let before: Vec<_> = retained
            .iter()
            .map(|path| fs::read(path).unwrap())
            .collect();
        fixture.begin(kind).unwrap().finish().unwrap();
        assert!(candidates.iter().all(|path| !path.exists()));
        for (path, bytes) in retained.iter().zip(before) {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }
}

#[test]
fn shared_operations_leases_and_inspection_never_sweep() {
    let _fork = crate::os::fork_guard().unwrap();
    let fixture = Fixture::new();
    let candidates: Vec<_> = std::iter::once("")
        .chain(LEAVES)
        .map(|leaf| fixture.candidate(leaf))
        .collect();
    for kind in [
        OperationKind::Status,
        OperationKind::Models,
        OperationKind::SteeringCanary,
    ] {
        fixture.begin(kind).unwrap().finish().unwrap();
    }
    drop(fixture.state.runtime_lease(&fixture.binding).unwrap());
    fixture.state.recovery_report(&fixture.binding).unwrap();
    fixture
        .state
        .lookup("main", &fixture.binding.profile)
        .unwrap();
    assert!(candidates.iter().all(|path| path.exists()));
}

#[test]
fn empty_profile_cleanup_does_not_create_optional_evidence_directories() {
    let _fork = crate::os::fork_guard().unwrap();
    let fixture = Fixture::new();
    let before: std::collections::BTreeSet<_> = fs::read_dir(&fixture.binding.directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    fixture
        .begin(OperationKind::Login)
        .unwrap()
        .finish()
        .unwrap();
    let after: std::collections::BTreeSet<_> = fs::read_dir(&fixture.binding.directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(before, after);
    assert!(
        LEAVES
            .iter()
            .all(|leaf| !fixture.binding.directory.join(leaf).exists())
    );
}

const STEPS: [Step; 4] = [
    Step::BeforeRemove,
    Step::AfterRemove,
    Step::BeforeDirectorySync,
    Step::AfterDirectorySync,
];

#[test]
fn root_and_evidence_cleanup_faults_are_retryable_and_empty_retry_syncs() {
    let _fork = crate::os::fork_guard().unwrap();
    for leaf in ["", "recovery-receipts"] {
        for at in STEPS {
            let fixture = Fixture::new();
            let candidate = fixture.candidate(leaf);
            let directory = fixture.directory(leaf);
            let target = if matches!(at, Step::BeforeRemove | Step::AfterRemove) {
                candidate.clone()
            } else {
                directory.clone()
            };
            let error = with_hook(
                move |path, step| {
                    if path == target && step == at {
                        Err(io::Error::from_raw_os_error(libc::EIO))
                    } else {
                        Ok(())
                    }
                },
                || fixture.begin(OperationKind::Login),
            )
            .err()
            .unwrap();
            assert_eq!(error.io_raw_os_error(), Some(libc::EIO));
            assert_eq!(candidate.exists(), at == Step::BeforeRemove);
            let observed = Arc::new(Mutex::new(Vec::new()));
            let events = observed.clone();
            with_hook(
                move |path, step| {
                    if path == directory {
                        events.lock().unwrap().push(step);
                    }
                    Ok(())
                },
                || fixture.begin(OperationKind::Login),
            )
            .unwrap()
            .finish()
            .unwrap();
            let events = observed.lock().unwrap();
            assert!(events.ends_with(&[Step::BeforeDirectorySync, Step::AfterDirectorySync]));
            assert!(!candidate.exists());
        }
    }
}
