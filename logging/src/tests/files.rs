use super::*;
use crate::{Admission, Level, Shutdown};
use std::{
    fs,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
};
#[test]
fn rotation_retains_five_bounded_files_with_private_whole_records() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    let config = Config {
        record_bytes: 1024,
        file_bytes: 5120,
        files: 5,
        ..Config::default()
    };
    let (logger, guard) = crate::guard::start(&path, config).unwrap();
    for i in 0..400 {
        assert_eq!(
            logger.record(Level::Info, "test.rotation", |f| f.count("sequence", i)),
            Admission::Queued
        );
    }
    assert_eq!(
        guard.finish(Duration::from_secs(10)).state,
        Shutdown::Drained
    );
    let files: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_none_or(|extension| extension != "lock"))
        .collect();
    assert_eq!(files.len(), 5);
    for file in &files {
        let meta = fs::metadata(file).unwrap();
        assert!(meta.len() <= 5120);
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        let bytes = fs::read(file).unwrap();
        let contents = atomic_fs::read_complete_lines(&bytes).unwrap();
        assert!(contents.incomplete_tail.is_empty());
        for line in contents.lines {
            serde_json::from_slice::<serde_json::Value>(line).unwrap();
        }
    }
    let mut raw = fs::OpenOptions::new()
        .append(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    raw.write_all(b"partial").unwrap();
    drop(raw);
    let (_, guard) = crate::guard::start(&path, Config::default()).unwrap();
    assert_eq!(
        guard.finish(Duration::from_secs(2)).state,
        Shutdown::Drained
    );
    let text = fs::read_to_string(path).unwrap();
    assert!(text.contains("log.recovered_tail"));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 6); // Five data files plus stable lock.
}
#[test]
fn competing_start_and_timed_out_shutdown_do_not_steal_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    let owner_path = dir.path().join("daemon.lock");
    let owner = fs::File::create(&owner_path).unwrap();
    owner.lock().unwrap();
    let contender = fs::OpenOptions::new()
        .write(true)
        .open(&owner_path)
        .unwrap();
    let sink =
        crate::sink::FileSink::open_owned(&path, &Config::default(), Box::new(owner)).unwrap();
    let (blocked, entered, release) = Blocked::new(sink);
    let (logger, guard) = start_with(
        Config::default(),
        Box::new(blocked),
        Box::new(io::sink()),
        &Threads,
    )
    .unwrap();
    assert_eq!(
        crate::guard::start(&path, Config::default())
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(
        logger.record(Level::Info, "still.owned", |_| Ok(())),
        Admission::Queued
    );
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(
        guard.finish(Duration::from_millis(10)).state,
        Shutdown::Unconfirmed
    );
    assert!(matches!(
        contender.try_lock(),
        Err(fs::TryLockError::WouldBlock)
    ));
    assert_eq!(
        crate::guard::start(&path, Config::default())
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::WouldBlock
    );
    release.send(()).unwrap();
    until(|| match crate::guard::start(&path, Config::default()) {
        Ok((_, guard)) => {
            assert_eq!(
                guard.finish(Duration::from_secs(2)).state,
                Shutdown::Drained
            );
            true
        }
        Err(error) => {
            assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
            false
        }
    });
    until(|| match contender.try_lock() {
        Ok(()) => true,
        Err(fs::TryLockError::WouldBlock) => false,
        Err(error) => panic!("Unexpected lock error: {error}"),
    });
    assert!(fs::read_to_string(path).unwrap().contains("still.owned"));
}
