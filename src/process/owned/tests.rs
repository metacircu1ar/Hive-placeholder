use super::*;

#[test]
fn poll_observes_readiness_and_errors_before_an_expired_deadline() {
    let deadline = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
    for expected in [Ok(true), Ok(false), Err("observation failed")] {
        let mut observations = 0;
        let actual = wait_until(deadline, Duration::from_secs(60), || {
            observations += 1;
            expected
        });
        assert_eq!(actual, expected);
        assert_eq!(observations, 1);
    }
}

#[test]
fn group_cleanup_retains_each_callers_receipt_policy() {
    for (policy, confirmed) in [
        (CleanupReceipt::GroupAbsence, true),
        (CleanupReceipt::Unconfirmed, false),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let state = crate::state::State::new(tmp.path().join("state")).unwrap();
        let mut operation = state.metadata_probe().unwrap();
        let launcher = crate::launch::test_cache().unwrap();
        let super::super::Spawned { child, receipt, .. } = super::super::spawn(
            &launcher,
            std::process::Command::new("/bin/sleep").arg("10"),
            &mut operation,
            true,
            None,
        )
        .unwrap();
        let pid = child.id();
        drop(OwnedGroup::new(child, receipt, policy));
        assert!(os::group_absent(pid).unwrap());
        assert_eq!(operation.children_confirmed(), confirmed);
    }
}
