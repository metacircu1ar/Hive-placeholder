use super::*;
use std::os::fd::{AsFd, AsRawFd};

#[test]
fn version_probe_retains_owner_and_leaves_status_and_bytes_to_the_caller() {
    let tmp = tempfile::tempdir().unwrap();
    let state = crate::state::State::new(tmp.path().join("state")).unwrap();
    let mut operation = state.metadata_probe().unwrap();
    let owner = tempfile::tempfile().unwrap();
    let mut command = Command::new("/bin/sh");
    command.args([
        "-c",
        "test -e /dev/fd/$1 || exit 17; printf 'caller bytes'; exit 7",
        "version-probe",
        &owner.as_raw_fd().to_string(),
    ]);
    let launcher = crate::launch::test_cache().unwrap();
    let observed = version_probe(
        &launcher,
        &mut command,
        &mut operation,
        Duration::from_secs(5),
        Cancellation::default(),
        Some(owner.as_fd()),
        |output, operation| {
            operation.verify_probe_ownership()?;
            Ok((output.status.code(), output.stdout))
        },
    )
    .unwrap();
    assert_eq!(observed, (Some(7), b"caller bytes".to_vec()));
    operation.finish().unwrap();
}
