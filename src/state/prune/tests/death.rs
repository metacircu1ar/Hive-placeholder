use super::{recovery::*, *};
use crate::state::sync_tests::with_hook;
use std::{cell::Cell, path::PathBuf, process::Stdio, time::Duration};

#[test]
fn process_death_at_each_prune_boundary_is_recoverable() {
    for index in 0..faults().len() + 1 + receipt_faults().len() {
        let fixture = {
            let _fork = crate::os::fork_guard().unwrap();
            Fixture::new()
        };
        let receipt = if index > faults().len() {
            let _fork = crate::os::fork_guard().unwrap();
            pause_receipt(&fixture);
            Some(std::fs::read(fixture.binding.directory.join("prune-receipt.json")).unwrap())
        } else {
            None
        };
        let mut command =
            crate::test_support::rerun_isolated("state::prune::tests::death::crash_fixture", true);
        command
            .env("HIVE_PRUNE_ROOT", &fixture.state.root)
            .env("HIVE_PRUNE_ID", fixture.binding.id.to_string())
            .env("HIVE_PRUNE_FAULT", index.to_string())
            .stdout(Stdio::null());
        let mut child = crate::test_support::ChildGuard(crate::os::spawn(&mut command).unwrap());
        let status = crate::test_support::wait_until(
            Duration::from_secs(10),
            Duration::from_millis(5),
            || child.try_wait().unwrap(),
        );
        assert_eq!(status.code(), Some(73), "boundary {index}");
        let _fork = crate::os::fork_guard().unwrap();
        if let Some(receipt) = receipt {
            assert_eq!(
                std::fs::read(fixture.binding.directory.join("prune-receipt.json")).unwrap(),
                receipt
            );
            assert_eq!(
                fixture
                    .state
                    .read_index()
                    .unwrap()
                    .generation(fixture.binding.id)
                    .unwrap()
                    .state,
                GenerationState::Pruning
            );
            assert!(!fixture.binding.directory.join("provider").exists());
        }
        fixture
            .prune()
            .unwrap_or_else(|error| panic!("boundary {index}: {error:?}"));
        fixture.completed();
    }
}
#[test]
#[ignore = "owned subprocess fixture exercised by process_death_at_each_prune_boundary"]
fn crash_fixture() {
    let state = State::new(PathBuf::from(std::env::var_os("HIVE_PRUNE_ROOT").unwrap())).unwrap();
    let id = std::env::var("HIVE_PRUNE_ID").unwrap().parse().unwrap();
    let index = std::env::var("HIVE_PRUNE_FAULT")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let partial = index == faults().len();
    let fault = if partial {
        Fault {
            leaf: "provider",
            step: atomic_fs::Step::BeforeRemove,
            occurrence: 1,
        }
    } else if index > faults().len() {
        receipt_faults()[index - faults().len() - 1]
    } else {
        faults()[index]
    };
    let seen = Cell::new(0);
    with_hook(
        move |path, step| {
            if selected(fault, &seen, path, step) {
                if partial {
                    std::fs::remove_file(path.with_file_name("provider.pruning").join("a"))?;
                }
                std::process::exit(73);
            }
            Ok(())
        },
        || state.prune("main", id),
    )
    .unwrap();
    panic!("death boundary not reached");
}
