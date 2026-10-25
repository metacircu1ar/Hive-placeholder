use super::*;
use crate::test_support::{rerun_isolated, run_isolated};
use std::{sync::mpsc::sync_channel, time::Duration};

#[test]
fn sqlite_admission_waits_for_raw_descriptor_close_and_keeps_transaction_locks() {
    let temporary = crate::runtime::test_directory();
    let path = temporary.path().join("runtime.sqlite");
    let (published, publication) = sync_channel(1);
    let (proceed, allowed) = sync_channel(1);
    let (finished, finish) = sync_channel(1);
    std::thread::scope(|scope| {
        let creator_path = path.clone();
        let creator = scope.spawn(move || {
            let observed = creator_path.clone();
            let store = with_hook(
                move |step| {
                    if step == Step::BeforePublish {
                        assert!(!observed.exists());
                    }
                    if step == Step::AfterPublish {
                        published.send(()).unwrap();
                        allowed.recv_timeout(Duration::from_secs(10)).unwrap();
                    }
                    Ok(())
                },
                || super::super::open(&creator_path),
            )
            .unwrap();
            // Close this SQLite-owned descriptor while a peer transaction lives.
            finish.recv_timeout(Duration::from_secs(10)).unwrap();
            drop(store);
        });
        publication.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(matches!(
            super::super::super::ADMISSION.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        let (starting, started) = sync_channel(1);
        let (admitted, admission) = sync_channel(1);
        let (release, released) = sync_channel(1);
        let reader_path = path.clone();
        let opener = scope.spawn(move || {
            starting.send(()).unwrap();
            let connection = super::super::open(&reader_path).unwrap();
            schema::validate(&connection).unwrap();
            connection.execute_batch("BEGIN IMMEDIATE").unwrap();
            admitted.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(10)).unwrap();
            connection.execute_batch("ROLLBACK").unwrap();
        });
        started.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(matches!(
            admission.recv_timeout(Duration::from_millis(100)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        proceed.send(()).unwrap();
        admission.recv_timeout(Duration::from_secs(10)).unwrap();
        finished.send(()).unwrap();
        creator.join().unwrap();
        run_isolated(
            rerun_isolated("runtime::store::tests::schema_inspection_lock_probe", true)
                .env("HIVE_SCHEMA_LOCK_PROBE", &path),
        );
        release.send(()).unwrap();
        opener.join().unwrap();
    });
}

#[test]
fn live_sqlite_preparation_keeps_daemon_ownership_until_publication_finishes() {
    let _fork = crate::os::fork_guard().unwrap();
    let temp = crate::runtime::test_directory();
    let state = crate::state::State::new(temp.path().join("state")).unwrap();
    drop(state.initialize().unwrap());
    let runtime = state.root.join("runtime");
    crate::state::directory(&runtime).unwrap();
    let lock_path = runtime.join("daemon.lock");
    let owner = crate::state::Lock::acquire(&lock_path).unwrap();
    let observed = runtime.clone();
    let published = with_hook(
        move |step| {
            if step == Step::Written {
                assert_eq!(
                    crate::state::Lock::acquire(&lock_path)
                        .err()
                        .unwrap()
                        .code(),
                    "profile_busy"
                );
                assert_eq!(
                    fs::read_dir(&observed)
                        .unwrap()
                        .filter(|e| hive_setup_support::names::state_preparation_name(
                            &e.as_ref().unwrap().file_name()
                        ))
                        .count(),
                    1
                );
            }
            Ok(())
        },
        || super::super::open(&runtime.join("runtime.sqlite")),
    )
    .unwrap();
    drop(published);
    drop(owner);
    let owner = crate::state::Lock::acquire(&runtime.join("daemon.lock")).unwrap();
    state.cleanup_preparations(&owner).unwrap();
    complete(&runtime.join("runtime.sqlite"));
}
