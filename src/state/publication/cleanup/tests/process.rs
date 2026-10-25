use super::*;
use crate::test_support::{ChildGuard, rerun_isolated, wait_until};
use std::{path::PathBuf, process::Stdio, time::Duration};

const STEPS: [Step; 4] = [
    Step::BeforeRemove,
    Step::AfterRemove,
    Step::BeforeDirectorySync,
    Step::AfterDirectorySync,
];

#[test]
fn process_death_during_cleanup_is_retryable_at_every_boundary() {
    for index in 0..STEPS.len() {
        let (_temp, state, owner) = fixture();
        let path = preparation(&state.root.join("runtime/output"));
        let committed = state.root.join("runtime/output/committed.json");
        state::write_json(&committed, &serde_json::json!({"keep":true})).unwrap();
        let before = fs::read(&committed).unwrap();
        drop(owner);
        crash(&state.root, index.to_string());
        assert_eq!(path.exists(), index == 0);
        let owner = Lock::acquire(&state.root.join("runtime/daemon.lock")).unwrap();
        state.cleanup_preparations(&owner).unwrap();
        assert!(!path.exists());
        assert_eq!(fs::read(&committed).unwrap(), before);
    }
}

#[test]
fn process_death_leaves_real_json_preparations_for_owned_retry() {
    for location in [
        "index.json",
        "runtime/record.json",
        "runtime/output/record.json",
    ] {
        let (_temp, state, owner) = fixture();
        drop(owner);
        let old = state.root.join(location);
        let before = crate::test_support::optional_bytes(&old);
        crash(&state.root, location.into());
        let directory = old.parent().unwrap();
        let pending: Vec<_> = fs::read_dir(directory)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(hive_setup_support::names::state_preparation_name)
            })
            .collect();
        assert_eq!(pending.len(), 1);
        let owner = Lock::acquire(&state.root.join("runtime/daemon.lock")).unwrap();
        state.cleanup_preparations(&owner).unwrap();
        assert!(!pending[0].exists());
        assert_eq!(crate::test_support::optional_bytes(&old), before);
    }
}

fn crash(root: &Path, mode: String) {
    let mut command = rerun_isolated(
        "state::publication::cleanup::tests::process::crash_fixture",
        true,
    );
    command
        .env("HIVE_SWEEP_ROOT", root)
        .env("HIVE_SWEEP_MODE", mode)
        .stdout(Stdio::null());
    let mut child = ChildGuard(crate::os::spawn(&mut command).unwrap());
    let status = wait_until(Duration::from_secs(10), Duration::from_millis(5), || {
        child.try_wait().unwrap()
    });
    assert_eq!(status.code(), Some(73));
}

#[test]
#[ignore = "owned process-death fixture; invoked by both cleanup recovery tests"]
fn crash_fixture() {
    let state = State::new(PathBuf::from(std::env::var_os("HIVE_SWEEP_ROOT").unwrap())).unwrap();
    let owner = Lock::acquire(&state.root.join("runtime/daemon.lock")).unwrap();
    let mode = std::env::var("HIVE_SWEEP_MODE").unwrap();
    if let Ok(index) = mode.parse::<usize>() {
        let directory = state.root.join("runtime/output");
        state::sync_tests::with_hook(
            move |path, step| {
                if (path == directory || path.parent() == Some(&directory)) && step == STEPS[index]
                {
                    std::process::exit(73);
                }
                Ok(())
            },
            || state.cleanup_preparations(&owner),
        )
        .unwrap();
    } else {
        let target = state.root.join(&mode);
        let observed = target.clone();
        let _index = (mode == "index.json").then(|| state.metadata_lock().unwrap());
        state::sync_tests::with_hook(
            move |path, step| {
                if path == observed && step == Step::Written {
                    std::process::exit(73);
                }
                Ok(())
            },
            || state::write_json(&target, &serde_json::json!({"new":true})),
        )
        .unwrap();
    }
    panic!("death boundary not reached");
}
