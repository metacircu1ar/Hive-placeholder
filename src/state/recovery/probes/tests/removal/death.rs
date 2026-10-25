use super::*;
use crate::test_support::{ChildGuard, rerun_isolated, wait_until};
use std::{process::Stdio, time::Duration};
#[test]
fn sweep_finishes_after_process_death_at_each_removal_boundary() {
    for index in 0..=faults().len() {
        let tmp = tempfile::tempdir().unwrap();
        let state = State::new(tmp.path().join("state")).unwrap();
        let mut command = rerun_isolated(
            "state::recovery::probes::tests::removal::death::crash_fixture",
            true,
        );
        command
            .env("HIVE_PROBE_REMOVAL_ROOT", &state.root)
            .env("HIVE_PROBE_REMOVAL_BOUNDARY", index.to_string())
            .stdout(Stdio::null());
        let mut child = ChildGuard(crate::os::spawn(&mut command).unwrap());
        let status = wait_until(Duration::from_secs(10), Duration::from_millis(5), || {
            child.try_wait().unwrap()
        });
        assert_eq!(status.code(), Some(73), "boundary {index}");
        let identity = state.metadata_root_identity().unwrap();
        if index == 0 {
            assert_eq!(super::super::temps::stages(&state).len(), 1);
            let cleanup = state.remove_empty_metadata_root(&identity);
            assert!(!cleanup.removed && cleanup.warning.is_some());
        }
        state.sweep_metadata_probes_checked().unwrap();
        assert_eq!(
            fs::read_dir(state.root.join("native-probes"))
                .unwrap()
                .count(),
            0,
            "boundary {index}"
        );
        let cleanup = state.remove_empty_metadata_root(&identity);
        assert!(
            cleanup.removed && cleanup.warning.is_none(),
            "boundary {index}"
        );
    }
}
#[test]
#[ignore = "owned subprocess fixture exercised by sweep_finishes_after_process_death"]
fn crash_fixture() {
    let state = State::new(std::env::var_os("HIVE_PROBE_REMOVAL_ROOT").unwrap().into()).unwrap();
    let index: usize = std::env::var("HIVE_PROBE_REMOVAL_BOUNDARY")
        .unwrap()
        .parse()
        .unwrap();
    let operation = state.metadata_probe().unwrap();
    let dir = fs::read_dir(state.root.join("native-probes"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let id = dir.file_name().unwrap().to_str().unwrap().parse().unwrap();
    drop(operation);
    let partial = index == faults().len();
    let fault = if partial {
        Fault {
            permit: false,
            step: Step::BeforeRemove,
            occurrence: 1,
        }
    } else {
        faults()[index]
    };
    let seen = Cell::new(0);
    with_hook(
        move |path, step| {
            if selected(fault, &seen, path, step) {
                if partial {
                    let aside = path.with_file_name(format!("{id}.pruning"));
                    fs::remove_file(aside.join("identity.json"))?;
                    fs::remove_file(aside.join("auth.lock"))?;
                }
                std::process::exit(73);
            }
            Ok(())
        },
        || finish_probe(&state, &dir, id),
    )
    .unwrap();
    panic!("death boundary not reached");
}
