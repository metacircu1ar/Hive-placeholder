use super::*;
use crate::test_support::{ChildGuard, rerun_isolated, wait_until};
use std::{
    io::{Read, Write},
    process::Stdio,
    time::Duration,
};
#[test]
fn old_inode_waiter_cannot_enter_after_empty_root_cleanup() {
    for replacement in [false, true] {
        let (_tmp, state) = fixture();
        let identity = state.metadata_root_identity().unwrap();
        let lock = publication_lock(&state).unwrap();
        let mut command = rerun_isolated(
            "state::recovery::probes::tests::wrapper::waiter::waiter_fixture",
            true,
        );
        command
            .env("HIVE_PROBE_WAITER_ROOT", &state.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut child = ChildGuard(crate::os::spawn(&mut command).unwrap());
        // The child signals only after opening the original inode, before flock.
        let output = child.stdout.take().unwrap();
        let (sent, received) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(output).lines() {
                if line.unwrap() == "opened" {
                    sent.send(()).unwrap();
                    break;
                }
            }
        });
        received.recv_timeout(Duration::from_secs(5)).unwrap();
        // Cleanup uses this lock domain. We emulate its unlink while retaining the
        // old lock, then run actual nonrecursive cleanup against the new domain.
        fs::remove_file(state.root.join("native-probes.lock")).unwrap();
        let cleanup = state.remove_empty_metadata_root(&identity);
        assert!(cleanup.removed && cleanup.warning.is_none());
        if replacement {
            state::directory(&state.root).unwrap();
            drop(publication_lock(&state).unwrap());
        }
        child.stdin.take().unwrap().write_all(b"x").unwrap();
        drop(lock);
        let status = wait_until(Duration::from_secs(5), Duration::from_millis(5), || {
            child.try_wait().unwrap()
        });
        assert_eq!(status.code(), Some(74));
        reader.join().unwrap();
        assert_eq!(state.root.exists(), replacement);
    }
}
#[test]
#[ignore = "owned subprocess fixture for old_inode_waiter_cannot_enter"]
fn waiter_fixture() {
    let state = State::new(std::env::var_os("HIVE_PROBE_WAITER_ROOT").unwrap().into()).unwrap();
    let result = with_hook(
        |path, step| {
            if path.file_name().unwrap() == "native-probes.lock" && step == Step::BeforePublish {
                println!("opened");
                std::io::stdout().flush()?;
                std::io::stdin().read_exact(&mut [0])?;
            }
            Ok(())
        },
        || publication_lock(&state),
    );
    std::process::exit(if result.is_err() { 74 } else { 75 });
}
