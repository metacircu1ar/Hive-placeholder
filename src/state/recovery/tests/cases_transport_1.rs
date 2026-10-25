//! Transport regressions.
use super::*;

#[test]
fn caller_death_at_each_launch_phase_preserves_availability_without_replaying_native_work() {
    use std::time::{Duration, Instant};
    for phase in ["intent", "bound", "started", "granted"] {
        let (_tmp, state, binding) = fixture();
        let id = Uuid::new_v4().to_string();
        let result = crate::os::spawn(
            crate::test_support::rerun_isolated(
                "state::recovery::tests::launch_crash_fixture",
                true,
            )
            .env("HIVE_RECOVERY_TEST_ROOT", &state.root)
            .env("HIVE_RECOVERY_TEST_PHASE", phase)
            .env("HIVE_RECOVERY_TEST_PROVIDER", &id)
            .stdout(std::process::Stdio::null()),
        )
        .and_then(|mut child| child.wait())
        .unwrap();
        assert_eq!(result.code(), Some(79));
        state.runtime_lease(&binding).unwrap();
        if phase == "granted" {
            let deadline = Instant::now() + Duration::from_secs(5);
            crate::test_support::wait_until(
                deadline.saturating_duration_since(Instant::now()),
                Duration::from_millis(10),
                || state.root.join("native.pid").exists().then_some(()),
            );
            let pid = fs::read_to_string(state.root.join("native.pid"))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            let identity = ProcessIdentity::read(pid).unwrap().unwrap();
            assert!(matches!(
                state.operation(&binding, crate::state::OperationKind::Login),
                Err(Error::ProfileBusy { .. })
            ));
            fs::write(state.root.join("native.stop"), "stop").unwrap();
            crate::test_support::wait_until(
                deadline.saturating_duration_since(Instant::now()),
                Duration::from_millis(10),
                || identity.absent().unwrap().then_some(()),
            );
        } else {
            if phase != "intent" {
                let shim: ProcessIdentity = read_json(&state.root.join("shim.json")).unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                crate::test_support::wait_until(
                    deadline.saturating_duration_since(Instant::now()),
                    Duration::from_millis(10),
                    || shim.absent().unwrap().then_some(()),
                );
            }
            assert!(!state.root.join("native.pid").exists());
        }
        assert_eq!(
            state.check_conversation(&binding, Some(&id)).is_ok(),
            matches!(phase, "intent" | "bound")
        );
    }
}
