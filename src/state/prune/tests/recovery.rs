use super::*;
use crate::{
    config::{Method, Profile, Provider},
    state::{self, sync_tests::with_hook},
};
use atomic_fs::Step;
use std::os::unix::fs::MetadataExt;
use std::{cell::Cell, error::Error as _, fs, io};

pub(super) struct Fixture {
    _temporary: tempfile::TempDir,
    pub(super) state: State,
    pub(super) binding: Binding,
    active: Value,
    lock: LockIdentity,
    audit: Vec<(std::path::PathBuf, Vec<u8>)>,
}
impl Fixture {
    pub(super) fn new() -> Self {
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
        // Make real audit evidence before retirement, not synthetic ownership data.
        state.recovery_report(&binding).unwrap();
        let repair = state.begin_repair("main", &binding.profile).unwrap();
        state.finish_repair(&repair, true, None).unwrap();
        let active = project(
            state
                .read_index()
                .unwrap()
                .generation(repair.binding.id)
                .unwrap(),
        );
        for tree in ["provider", "work"] {
            for name in ["a", "b"] {
                state::write_json(
                    &binding.directory.join(tree).join(name),
                    &serde_json::json!({"secret": name}),
                )
                .unwrap();
            }
        }
        let lock = LockIdentity::at(&binding.directory.join("auth.lock")).unwrap();
        let mut audit = vec![binding.directory.join("identity.json")];
        audit.extend(
            fs::read_dir(binding.directory.join("recovery-receipts"))
                .unwrap()
                .map(|entry| entry.unwrap().path()),
        );
        let audit = audit
            .into_iter()
            .map(|path| {
                let bytes = fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect();
        Self {
            _temporary: temporary,
            state,
            binding,
            active,
            lock,
            audit,
        }
    }
    pub(super) fn prune(&self) -> Result<Value> {
        self.state.prune("main", self.binding.id)
    }
    pub(super) fn unchanged(&self) {
        assert_eq!(
            LockIdentity::at(&self.binding.directory.join("auth.lock")).unwrap(),
            self.lock
        );
        for (path, bytes) in &self.audit {
            assert_eq!(&fs::read(path).unwrap(), bytes);
        }
        let index = self.state.read_index().unwrap();
        assert_eq!(
            project(index.generation(index.profiles["main"].id).unwrap()),
            self.active
        );
    }
    pub(super) fn completed(&self) {
        self.unchanged();
        let index = self.state.read_index().unwrap();
        let generation = index.generation(self.binding.id).unwrap();
        assert_eq!(generation.state, GenerationState::Pruned);
        assert!(!generation.authenticated && !generation.possibly_authenticated);
        for tree in ["provider", "work"] {
            assert_eq!(
                fs::read_dir(self.binding.directory.join(tree))
                    .unwrap()
                    .count(),
                0
            );
            assert!(
                !self
                    .binding
                    .directory
                    .join(format!("{tree}.pruning"))
                    .exists()
            );
        }
    }
    pub(super) fn fenced(&self) {
        for kind in [
            OperationKind::Login,
            OperationKind::Status,
            OperationKind::Recover,
        ] {
            assert!(self.state.operation(&self.binding, kind).is_err());
        }
        assert!(self.state.runtime_lease(&self.binding).is_err());
    }
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Fault {
    pub leaf: &'static str,
    pub step: Step,
    pub occurrence: usize,
}
pub(super) fn faults() -> Vec<Fault> {
    let mut faults = Vec::new();
    for leaf in ["index.json", "prune-intent.json", "prune-receipt.json"] {
        for step in [
            Step::BeforeFileSync,
            Step::AfterFileSync,
            Step::BeforeDirectorySync,
            Step::AfterDirectorySync,
        ] {
            faults.push(Fault {
                leaf,
                step,
                occurrence: 1,
            });
            if matches!(leaf, "index.json" | "prune-intent.json") {
                faults.push(Fault {
                    leaf,
                    step,
                    occurrence: 2,
                });
            }
        }
        if leaf != "index.json" {
            for step in [Step::Written, Step::BeforePublish, Step::AfterPublish] {
                faults.push(Fault {
                    leaf,
                    step,
                    occurrence: 1,
                });
            }
        }
    }
    for leaf in ["provider", "work"] {
        for step in [
            Step::BeforePublish,
            Step::AfterPublish,
            Step::BeforeRemove,
            Step::AfterRemove,
            Step::BeforeDirectorySync,
            Step::AfterDirectorySync,
        ] {
            faults.push(Fault {
                leaf,
                step,
                occurrence: 1,
            });
            if matches!(
                step,
                Step::BeforePublish
                    | Step::AfterPublish
                    | Step::BeforeDirectorySync
                    | Step::AfterDirectorySync
            ) {
                faults.push(Fault {
                    leaf,
                    step,
                    occurrence: 2,
                });
            }
            if matches!(step, Step::BeforeDirectorySync | Step::AfterDirectorySync) {
                faults.push(Fault {
                    leaf,
                    step,
                    occurrence: 3,
                });
            }
        }
    }
    faults
}
pub(super) fn selected(fault: Fault, seen: &Cell<usize>, path: &Path, step: Step) -> bool {
    if path.file_name().unwrap() != fault.leaf || step != fault.step {
        return false;
    }
    seen.set(seen.get() + 1);
    seen.get() == fault.occurrence
}
pub(super) fn fail(fixture: &Fixture, fault: Fault) {
    let seen = Cell::new(0);
    let error = with_hook(
        move |path, step| {
            if selected(fault, &seen, path, step) {
                Err(io::Error::from_raw_os_error(libc::EIO))
            } else {
                Ok(())
            }
        },
        || fixture.prune(),
    )
    .unwrap_err();
    assert_eq!(
        native_cause(&error),
        Some(libc::EIO),
        "{fault:?}: {error:?}"
    );
}

#[test]
fn every_prune_barrier_is_recoverable_with_original_lock_and_audit() {
    let _fork = crate::os::fork_guard().unwrap();
    for fault in faults() {
        let fixture = Fixture::new();
        fail(&fixture, fault);
        fixture.unchanged();
        if fixture
            .state
            .read_index()
            .unwrap()
            .generation(fixture.binding.id)
            .unwrap()
            .state
            != GenerationState::Retired
        {
            fixture.fenced();
        }
        let path = fixture.binding.directory.join("prune-receipt.json");
        let receipt = crate::test_support::optional_bytes(&path);
        fixture
            .prune()
            .unwrap_or_else(|error| panic!("retry {fault:?}: {error:?}"));
        if let Some(receipt) = receipt {
            assert_eq!(fs::read(&path).unwrap(), receipt);
        }
        fixture.completed();
    }
}
#[test]
fn repeated_success_preserves_receipt_and_empty_directory_identities() {
    let _fork = crate::os::fork_guard().unwrap();
    let fixture = Fixture::new();
    let first = fixture.prune().unwrap();
    let receipt = fs::read(fixture.binding.directory.join("prune-receipt.json")).unwrap();
    let identities = || {
        ["provider", "work"].map(|name| {
            fs::metadata(fixture.binding.directory.join(name))
                .unwrap()
                .ino()
        })
    };
    let before = identities();
    let result = with_hook(
        |_, step| {
            assert_ne!(step, Step::BeforeRemove);
            Ok(())
        },
        || fixture.prune(),
    )
    .unwrap();
    assert_eq!(result, first);
    assert_eq!(identities(), before);
    assert_eq!(
        fs::read(fixture.binding.directory.join("prune-receipt.json")).unwrap(),
        receipt
    );
    fixture.completed();
    fixture.fenced();
}
#[test]
fn partial_deletion_and_failed_absence_sync_never_publish_a_receipt() {
    let _fork = crate::os::fork_guard().unwrap();
    let fixture = Fixture::new();
    let dir = fixture.binding.directory.clone();
    let error = with_hook(
        move |path, step| {
            if path.file_name().unwrap() == "provider" && step == Step::BeforeRemove {
                assert!(!path.exists());
                fs::remove_file(dir.join("provider.pruning/a"))?;
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
            Ok(())
        },
        || fixture.prune(),
    )
    .unwrap_err();
    assert_eq!(native_cause(&error), Some(libc::EIO));
    assert!(
        fixture
            .binding
            .directory
            .join("provider.pruning/b")
            .exists()
    );
    assert!(
        !fixture
            .binding
            .directory
            .join("prune-receipt.json")
            .exists()
    );
    fail(
        &fixture,
        Fault {
            leaf: "provider",
            step: Step::BeforeDirectorySync,
            occurrence: 2,
        },
    );
    assert!(!fixture.binding.directory.join("provider.pruning").exists());
    fail(
        &fixture,
        Fault {
            leaf: "provider",
            step: Step::BeforeDirectorySync,
            occurrence: 1,
        },
    );
    assert!(
        !fixture
            .binding
            .directory
            .join("prune-receipt.json")
            .exists()
    );
    fixture.prune().unwrap();
    fixture.completed();
}

pub(super) fn native_cause(error: &Error) -> Option<i32> {
    let mut current = error.source();
    while let Some(cause) = current {
        if let Some(io) = cause.downcast_ref::<io::Error>() {
            return io.raw_os_error();
        }
        current = cause.source();
    }
    None
}

pub(super) fn receipt_faults() -> Vec<Fault> {
    [
        Step::BeforeFileSync,
        Step::AfterFileSync,
        Step::BeforeDirectorySync,
        Step::AfterDirectorySync,
    ]
    .into_iter()
    .map(|step| Fault {
        leaf: "prune-receipt.json",
        step,
        occurrence: 1,
    })
    .collect()
}
pub(super) fn pause_receipt(fixture: &Fixture) {
    fail(
        fixture,
        Fault {
            leaf: "prune-receipt.json",
            step: Step::AfterPublish,
            occurrence: 1,
        },
    );
}
#[test]
fn a_retained_receipt_is_durable_before_reconstruction_or_terminal_acknowledgement() {
    let _fork = crate::os::fork_guard().unwrap();
    for terminal in [false, true] {
        for fault in receipt_faults() {
            let fixture = Fixture::new();
            if terminal {
                fixture.prune().unwrap();
            } else {
                pause_receipt(&fixture);
            }
            let receipt = fs::read(fixture.binding.directory.join("prune-receipt.json")).unwrap();
            fail(&fixture, fault);
            let phase = fixture
                .state
                .read_index()
                .unwrap()
                .generation(fixture.binding.id)
                .unwrap()
                .state;
            assert_eq!(
                phase,
                if terminal {
                    GenerationState::Pruned
                } else {
                    GenerationState::Pruning
                }
            );
            for name in ["provider", "work"] {
                assert_eq!(fixture.binding.directory.join(name).exists(), terminal);
            }
            assert_eq!(
                fs::read(fixture.binding.directory.join("prune-receipt.json")).unwrap(),
                receipt
            );
            fixture.prune().unwrap();
            fixture.completed();
        }
    }
}
