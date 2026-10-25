//! Captured probe timing at the deadline boundary.
use super::*;

#[test]
fn an_exit_observed_after_the_deadline_still_returns_the_output() {
    let tmp = tempfile::tempdir().unwrap();
    let state = crate::state::State::new(tmp.path().join("state")).unwrap();
    let mut operation = state.metadata_probe().unwrap();
    let launcher = crate::launch::test_cache().unwrap();
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "echo finished"]);
    let mut child = Captured::start(
        &launcher,
        &mut command,
        &mut operation,
        Duration::from_secs(30),
        Cancellation::install().unwrap(),
    )
    .unwrap();
    while !has_exited(&child.owned.child).unwrap() {
        thread::sleep(CAPTURE_POLL_INTERVAL);
    }
    // The process finished in time; only the observation comes late.
    child.deadline = Instant::now();
    let output = child.output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"finished\n");
}

#[test]
fn a_running_process_past_its_deadline_times_out() {
    let tmp = tempfile::tempdir().unwrap();
    let state = crate::state::State::new(tmp.path().join("state")).unwrap();
    let mut operation = state.metadata_probe().unwrap();
    let launcher = crate::launch::test_cache().unwrap();
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "exec sleep 30"]);
    let mut child = Captured::start(
        &launcher,
        &mut command,
        &mut operation,
        Duration::from_millis(200),
        Cancellation::install().unwrap(),
    )
    .unwrap();
    assert!(matches!(child.output(), Err(Error::Timeout { .. })));
}
