use super::*;
use crate::test_support::{ChildGuard, rerun_isolated, wait_until};
use std::{process::Stdio, time::Duration};

#[test]
fn owned_process_death_at_cleanup_boundaries_is_retryable() {
    for index in 0..STEPS.len() {
        let (_tmp, state) = fixture();
        let path = preparation(&state);
        let mut command = rerun_isolated(
            "state::recovery::probes::tests::temps::death::crash_fixture",
            true,
        );
        command
            .env("HIVE_PROBE_TEMP_ROOT", &state.root)
            .env("HIVE_PROBE_TEMP_BOUNDARY", index.to_string())
            .stdout(Stdio::null());
        let mut child = ChildGuard(crate::os::spawn(&mut command).unwrap());
        let status = wait_until(Duration::from_secs(10), Duration::from_millis(5), || {
            child.try_wait().unwrap()
        });
        assert_eq!(status.code(), Some(73), "{index}");
        assert_eq!(path.exists(), index == 0);
        let identity = state.metadata_root_identity().unwrap();
        state.sweep_metadata_probes_checked().unwrap();
        assert!(!path.exists());
        let cleanup = state.remove_empty_metadata_root(&identity);
        assert!(cleanup.removed && cleanup.warning.is_none());
    }
}

#[test]
#[ignore = "owned subprocess fixture exercised by owned_process_death_at_cleanup_boundaries"]
fn crash_fixture() {
    let state = State::new(std::env::var_os("HIVE_PROBE_TEMP_ROOT").unwrap().into()).unwrap();
    let index: usize = std::env::var("HIVE_PROBE_TEMP_BOUNDARY")
        .unwrap()
        .parse()
        .unwrap();
    with_hook(
        move |_, step| {
            if step == STEPS[index] {
                std::process::exit(73);
            }
            Ok(())
        },
        || state.sweep_metadata_probes_checked(),
    )
    .unwrap();
    panic!("cleanup death boundary not reached");
}
