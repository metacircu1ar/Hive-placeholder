use super::*;
use crate::test_support::{ChildGuard, rerun_isolated, wait_until};
use std::{cell::Cell, process::Stdio, time::Duration};

#[test]
fn death_at_every_boundary_leaves_a_resumable_outbox_and_no_stale_manifest() {
    for at in 0..STEPS.len() {
        let temporary = crate::runtime::test_directory();
        drop(seed(temporary.path()));
        let mut command = rerun_isolated(
            "runtime::store::artifacts::deletion::tests::process::crash_fixture",
            true,
        );
        command
            .env("HIVE_DELETE_CRASH_ROOT", temporary.path())
            .env("HIVE_DELETE_CRASH_BOUNDARY", at.to_string())
            .stdout(Stdio::null());
        let mut child = ChildGuard(crate::os::spawn(&mut command).unwrap());
        let status = wait_until(Duration::from_secs(10), Duration::from_millis(5), || {
            child.try_wait().unwrap()
        });
        assert_eq!(
            status.code(),
            Some(73),
            "fixture did not reach boundary {at}"
        );
        interrupted(temporary.path(), at);
    }
}

#[test]
#[ignore = "owned subprocess fixture; exercised at all boundaries by death_at_every_boundary"]
fn crash_fixture() {
    let root = PathBuf::from(std::env::var_os("HIVE_DELETE_CRASH_ROOT").unwrap());
    let at: usize = std::env::var("HIVE_DELETE_CRASH_BOUNDARY")
        .unwrap()
        .parse()
        .unwrap();
    let store = Store::open(&root.join("runtime.sqlite")).unwrap();
    let count = Cell::new(0);
    with_hook(
        move |step| {
            let index = count.get();
            assert_eq!(step, STEPS[index]);
            count.set(index + 1);
            if index == at {
                std::process::exit(73);
            }
            Ok(())
        },
        || store.finish_artifact_deletions(),
    )
    .unwrap();
    panic!("crash boundary was not reached");
}
