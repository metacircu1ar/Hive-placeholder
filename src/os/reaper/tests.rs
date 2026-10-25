//! Orphans adopted by a subreaper are reaped; children it spawned never are.
//! Fixtures run in their own process: adoption and reaping are process-wide.
use super::*;
use crate::test_support::{rerun_isolated, run_isolated};
use std::{
    io::Read,
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn pruning_keeps_unknown_births_and_unreadable_owned_children() {
    let observed = |start| {
        Ok(Some(Stat {
            state: b'Z',
            parent: 1,
            start,
        }))
    };
    assert!(retain_owned(0, observed(99)));
    assert!(retain_owned(99, observed(99)));
    for code in [libc::EMFILE, libc::EACCES, libc::EIO] {
        assert!(retain_owned(99, Err(io::Error::from_raw_os_error(code))));
    }
    assert!(!retain_owned(99, Ok(None)));
    assert!(!retain_owned(99, observed(100)));
}

/// Start `sh` that backgrounds a short `sleep` and exits at once, leaving the
/// sleep orphaned. Returns the shell (already reaped) and the orphan's PID.
fn orphaning_shell(group: bool) -> (std::process::Child, u32) {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "sleep 0.3 & echo $!"])
        .stdout(Stdio::piped());
    if group {
        command.process_group(0);
    }
    let mut shell = crate::os::spawn(&mut command).unwrap();
    let mut pid = String::new();
    shell
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut pid)
        .unwrap();
    assert!(shell.wait().unwrap().success());
    (shell, pid.trim().parse().unwrap())
}
fn gone(pid: u32) -> bool {
    stat(pid).is_none()
}

#[test]
fn an_adopted_orphan_in_a_checked_group_is_reaped() {
    run_isolated(&mut rerun_isolated(
        "os::reaper::tests::checked_group_fixture",
        true,
    ));
}
#[test]
#[ignore = "isolated subreaper fixture"]
fn checked_group_fixture() {
    crate::os::group::adopt_children().unwrap();
    let (shell, orphan) = orphaning_shell(true);
    // The orphan is now our zombie once it exits; the group stays present
    // until it is reaped.
    let confirmed = crate::process::wait_group_absent(shell.id()).unwrap();
    assert!(confirmed, "an adopted zombie kept the group present");
    assert!(gone(orphan));
}

#[test]
fn the_sweep_reaps_adopted_orphans_but_never_spawned_children() {
    run_isolated(&mut rerun_isolated(
        "os::reaper::tests::sweep_fixture",
        true,
    ));
}
#[test]
#[ignore = "isolated subreaper fixture"]
fn sweep_fixture() {
    crate::os::group::adopt_children().unwrap();
    let (_shell, orphan) = orphaning_shell(false);
    // A spawned child that exits stays waitable for its owner.
    let mut owned = crate::os::spawn(&mut Command::new("/bin/true")).unwrap();
    while !crate::os::has_exited(&owned).unwrap() {
        std::thread::sleep(Duration::from_millis(10));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while !gone(orphan) {
        assert!(
            Instant::now() < deadline,
            "the adopted orphan was never reaped"
        );
        reap_adopted().unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(owned.wait().unwrap().success());
}
