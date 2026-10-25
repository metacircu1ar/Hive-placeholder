use super::*;
use crate::runtime::{compact, compact_preview, maintenance_lock};
use crate::test_support::{ChildGuard, rerun_isolated, wait_until};
use std::{process::Stdio, sync::mpsc, time::Duration};

#[test]
fn actual_compaction_crashes_are_recoverable_and_only_checked_startup_sweeps() {
    for index in 0..STEPS.len() {
        let (temp, store) = {
            let _fork = crate::os::fork_guard().unwrap();
            fixture()
        };
        drop(store);
        let runtime = temp.path().join("runtime");
        let mut command = rerun_isolated(
            "runtime::store::maintenance::manifest::tests::process::crash_fixture",
            true,
        );
        command
            .env("HIVE_MANIFEST_CRASH_ROOT", temp.path())
            .env("HIVE_MANIFEST_CRASH_STEP", index.to_string())
            .stdout(Stdio::null());
        let mut child = ChildGuard(crate::os::spawn(&mut command).unwrap());
        let status = wait_until(Duration::from_secs(10), Duration::from_millis(5), || {
            child.try_wait().unwrap()
        });
        assert_eq!(status.code(), Some(73), "boundary {index}");
        let _fork = crate::os::fork_guard().unwrap();
        assert_eq!(
            fs::read(target(&runtime)).unwrap(),
            if index >= 4 { NEW } else { OLD }
        );
        let preparations = stages(&runtime);
        assert_eq!(preparations.len(), usize::from(index < 4));
        for path in &preparations {
            assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o600);
        }
        compact_preview(temp.path(), 0, 1).unwrap();
        compact(temp.path(), 0, 1).unwrap();
        assert_eq!(fs::read(target(&runtime)).unwrap(), NEW);
        assert!(preparations.iter().all(|p| p.exists()));
        let owner = maintenance_lock(temp.path()).unwrap();
        state::State::new(temp.path().to_owned())
            .unwrap()
            .cleanup_preparations(&owner)
            .unwrap();
        assert!(stages(&runtime).is_empty());
        assert_eq!(fs::read(target(&runtime)).unwrap(), NEW);
    }
}

#[test]
#[ignore = "owned process-exit fixture exercised by manifest compaction crash tests"]
fn crash_fixture() {
    let root = PathBuf::from(std::env::var_os("HIVE_MANIFEST_CRASH_ROOT").unwrap());
    let index: usize = std::env::var("HIVE_MANIFEST_CRASH_STEP")
        .unwrap()
        .parse()
        .unwrap();
    let expected = target(&root.join("runtime"));
    state::sync_tests::with_hook(
        move |path, step| {
            if path == expected && step == STEPS[index] {
                std::process::exit(73);
            }
            Ok(())
        },
        || compact(&root, 0, 1),
    )
    .unwrap();
    panic!("manifest crash boundary not reached");
}

#[test]
fn actual_manifest_owner_excludes_cleanup_and_readers_see_complete_rows() {
    let _fork = crate::os::fork_guard().unwrap();
    let (temp, store) = fixture();
    drop(store);
    let root = temp.path().to_owned();
    let runtime = root.join("runtime");
    let expected = target(&runtime);
    let (arrived, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        state::sync_tests::with_hook(
            move |path, step| {
                if path == expected {
                    arrived.send(step).unwrap();
                    released.recv_timeout(Duration::from_secs(10)).unwrap();
                }
                Ok(())
            },
            || compact(&root, 0, 1),
        )
        .unwrap();
    });
    for (index, expected) in STEPS.into_iter().enumerate() {
        assert_eq!(
            waiting.recv_timeout(Duration::from_secs(10)).unwrap(),
            expected
        );
        assert_eq!(
            maintenance_lock(temp.path()).err().unwrap().code(),
            "daemon_busy"
        );
        assert_eq!(
            fs::read(target(&runtime)).unwrap(),
            if index >= 4 { NEW } else { OLD }
        );
        assert_eq!(stages(&runtime).len(), usize::from(index < 4));
        release.send(()).unwrap();
    }
    writer.join().unwrap();
    assert_eq!(fs::read(target(&runtime)).unwrap(), NEW);
}
