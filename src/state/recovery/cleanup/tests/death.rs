use super::*;
use crate::test_support::{ChildGuard, rerun_isolated, wait_until};
use std::{process::Stdio, time::Duration};

fn crash(fixture: &Fixture, mode: &str, leaf: &str, index: usize) {
    let mut command = rerun_isolated(
        "state::recovery::cleanup::tests::death::crash_fixture",
        true,
    );
    command
        .env("HIVE_CREDENTIAL_SWEEP_ROOT", &fixture.state.root)
        .env("HIVE_CREDENTIAL_SWEEP_MODE", mode)
        .env("HIVE_CREDENTIAL_SWEEP_LEAF", leaf)
        .env("HIVE_CREDENTIAL_SWEEP_STEP", index.to_string())
        .stdout(Stdio::null());
    let mut child = ChildGuard(crate::os::spawn(&mut command).unwrap());
    let status = wait_until(Duration::from_secs(10), Duration::from_millis(5), || {
        child.try_wait().unwrap()
    });
    assert_eq!(status.code(), Some(73), "{mode}/{leaf}/{index}");
}

#[test]
fn actual_publisher_death_leaves_preparations_reclaimed_after_admission() {
    for leaf in ["", "executions", "recovery-receipts"] {
        let fixture = {
            let _fork = crate::os::fork_guard().unwrap();
            Fixture::new()
        };
        let before = fs::read(fixture.binding.directory.join("identity.json")).unwrap();
        crash(&fixture, "publish", leaf, 0);
        let _fork = crate::os::fork_guard().unwrap();
        let directory = fixture.binding.directory.join(leaf);
        let candidates: Vec<_> = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(hive_setup_support::names::state_preparation_name)
            })
            .collect();
        assert_eq!(candidates.len(), 1, "{leaf}");
        fixture
            .begin(OperationKind::Login)
            .unwrap()
            .finish()
            .unwrap();
        assert!(!candidates[0].exists());
        assert_eq!(
            fs::read(fixture.binding.directory.join("identity.json")).unwrap(),
            before
        );
    }
}

#[test]
fn cleanup_process_death_at_each_boundary_is_retryable() {
    for leaf in ["", "recovery-receipts"] {
        for index in 0..STEPS.len() {
            let fixture = {
                let _fork = crate::os::fork_guard().unwrap();
                Fixture::new()
            };
            let candidate = fixture.candidate(leaf);
            crash(&fixture, "cleanup", leaf, index);
            let _fork = crate::os::fork_guard().unwrap();
            assert_eq!(candidate.exists(), index == 0);
            fixture
                .begin(OperationKind::Login)
                .unwrap()
                .finish()
                .unwrap();
            assert!(!candidate.exists());
        }
    }
}

#[test]
#[ignore = "owned subprocess fixture exercised by credential publication/cleanup death tests"]
fn crash_fixture() {
    let state = State::new(
        std::env::var_os("HIVE_CREDENTIAL_SWEEP_ROOT")
            .unwrap()
            .into(),
    )
    .unwrap();
    let binding = state
        .lookup(
            "main",
            &Profile {
                provider: Provider::Codex,
                method: Method::Subscription,
                api_key_env: None,
                api_key_file: None,
            },
        )
        .unwrap()
        .unwrap();
    let leaf = std::env::var("HIVE_CREDENTIAL_SWEEP_LEAF").unwrap();
    let publish = std::env::var("HIVE_CREDENTIAL_SWEEP_MODE").unwrap() == "publish";
    let index: usize = std::env::var("HIVE_CREDENTIAL_SWEEP_STEP")
        .unwrap()
        .parse()
        .unwrap();
    let directory = binding.directory.join(&leaf);
    with_hook(
        move |path, step| {
            let selected = if publish {
                path.parent() == Some(directory.as_path()) && step == Step::Written
            } else {
                step == STEPS[index]
                    && (path == directory || path == directory.join(".hive-tmp-state-ABC123"))
            };
            if selected {
                std::process::exit(73);
            }
            Ok(())
        },
        || {
            if !publish {
                state.operation(&binding, OperationKind::Login).unwrap();
            } else if leaf.is_empty() {
                let _index = state.metadata_lock().unwrap();
                State::initialize_binding(&binding).unwrap();
            } else if leaf == "executions" {
                Arc::new(state.runtime_lease(&binding).unwrap())
                    .begin_provider(&Uuid::new_v4().to_string())
                    .unwrap();
            } else {
                state.recovery_report(&binding).unwrap();
            }
        },
    );
    panic!("credential cleanup crash boundary not reached");
}
