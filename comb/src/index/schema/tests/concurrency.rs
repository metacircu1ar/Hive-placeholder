use super::*;
use std::{sync::mpsc::sync_channel, time::Duration};

#[test]
fn all_native_admission_waits_for_publication_descriptor_close_and_keeps_sqlite_locks() {
    let directory = directory();
    let path = directory.path().join("index.sqlite");
    let (arrived, arrival) = sync_channel(1);
    let (release, released) = sync_channel(1);
    let (finish, finished) = sync_channel(1);
    std::thread::scope(|scope| {
        let creator_path = path.clone();
        let creator = scope.spawn(move || {
            let index = hooks::with(
                move |path, step| {
                    if path.file_name().unwrap() == "index.sqlite" && step == Step::AfterPublish {
                        arrived.send(()).unwrap();
                        released.recv_timeout(Duration::from_secs(10)).unwrap();
                    }
                    Ok(())
                },
                || Index::open(&creator_path),
            )
            .unwrap();
            finished.recv_timeout(Duration::from_secs(10)).unwrap();
            drop(index);
        });
        arrival.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(matches!(
            ADMISSION.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        let (admitted, admission) = sync_channel(3);
        let (end, ended) = sync_channel(1);
        let writer_path = path.clone();
        let writer_admitted = admitted.clone();
        let writer = scope.spawn(move || {
            let index = Index::open(&writer_path).unwrap();
            index.connection.execute_batch("BEGIN IMMEDIATE").unwrap();
            writer_admitted.send("writer").unwrap();
            ended.recv_timeout(Duration::from_secs(10)).unwrap();
            index.connection.execute_batch("ROLLBACK").unwrap();
        });
        let reader_path = path.clone();
        let reader_admitted = admitted.clone();
        let reader = scope.spawn(move || {
            drop(Index::reader(&reader_path, None).unwrap());
            reader_admitted.send("reader").unwrap();
        });
        let inspect_path = path.clone();
        let inspector = scope.spawn(move || {
            assert!(Index::existing_space(&inspect_path).unwrap().is_none());
            admitted.send("inspection").unwrap();
        });
        assert!(admission.recv_timeout(Duration::from_millis(100)).is_err());
        release.send(()).unwrap();
        let mut observed = Vec::new();
        for _ in 0..3 {
            observed.push(admission.recv_timeout(Duration::from_secs(10)).unwrap());
        }
        observed.sort_unstable();
        assert_eq!(observed, ["inspection", "reader", "writer"]);
        reader.join().unwrap();
        inspector.join().unwrap();
        finish.send(()).unwrap();
        creator.join().unwrap();
        super::process::child(directory.path(), "lock", 0);
        end.send(()).unwrap();
        writer.join().unwrap();
    });
}
