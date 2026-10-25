use super::*;
use std::{sync::mpsc::sync_channel, time::Duration};

#[test]
fn root_cleanup_waits_for_an_actual_index_publisher() {
    let (_temp, state, owner) = fixture();
    let root = state.root.clone();
    let (ready, prepared) = sync_channel(1);
    let (release, released) = sync_channel(1);
    let (arrive, arrived) = sync_channel(1);
    let (done, completed) = sync_channel(1);
    std::thread::scope(|scope| {
        let writer = scope.spawn(move || {
            let state = State::new(root.clone()).unwrap();
            let index = root.join("index.json");
            state::sync_tests::with_hook(
                move |path, step| {
                    if path == index && step == Step::Written {
                        ready.send(()).unwrap();
                        released.recv_timeout(Duration::from_secs(10)).unwrap();
                    }
                    Ok(())
                },
                || state.update_index(|transaction| transaction.save()),
            )
            .unwrap();
        });
        prepared.recv_timeout(Duration::from_secs(10)).unwrap();
        let candidates: Vec<_> = fs::read_dir(&state.root)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(hive_setup_support::names::state_preparation_name)
            })
            .collect();
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            Lock::acquire(&state.root.join("index.lock"))
                .err()
                .unwrap()
                .code(),
            "profile_busy"
        );
        let index_lock = state.root.join("index.lock");
        let cleaner = scope.spawn(|| {
            let result = state::sync_tests::with_hook(
                move |path, step| {
                    if path == index_lock && step == Step::BeforePublish {
                        arrive.send(()).unwrap();
                    }
                    Ok(())
                },
                || state.cleanup_preparations(&owner),
            );
            done.send(result).unwrap();
        });
        arrived.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(candidates[0].exists());
        let premature = completed.recv_timeout(Duration::from_millis(100));
        release.send(()).unwrap();
        assert!(matches!(
            premature,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        completed
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        writer.join().unwrap();
        cleaner.join().unwrap();
        assert!(!candidates[0].exists());
        state.read_index().unwrap();
    });
}

#[test]
fn active_output_publication_keeps_its_preparation_until_the_writer_finishes() {
    let (_temp, state, owner) = fixture();
    let target = state.root.join("runtime/output/turn.json");
    let directory = target.parent().unwrap().to_owned();
    let lock = state.root.join("runtime/daemon.lock");
    state::sync_tests::with_hook(
        move |_, step| {
            if step == Step::Written {
                assert_eq!(Lock::acquire(&lock).err().unwrap().code(), "profile_busy");
                assert_eq!(
                    fs::read_dir(&directory)
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
        || state::write_json(&target, &serde_json::json!({"result":"complete"})),
    )
    .unwrap();
    let expected = fs::read(&target).unwrap();
    state.cleanup_preparations(&owner).unwrap();
    assert_eq!(fs::read(&target).unwrap(), expected);
}
