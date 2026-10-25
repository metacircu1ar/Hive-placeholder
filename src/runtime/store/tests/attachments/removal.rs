use super::*;
use crate::{
    runtime::attachments::{Attachment, removal_boundaries},
    test_support::{ChildGuard, rerun_isolated, wait_until},
};
use std::{
    io,
    path::PathBuf,
    process::Stdio,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Fixture {
    _temp: tempfile::TempDir,
    database: PathBuf,
    source: PathBuf,
    private: PathBuf,
    key: String,
    target: PathBuf,
}
fn fixture() -> (Fixture, Store) {
    let temp = crate::runtime::test_directory();
    let private = temp.path().join("state");
    state::directory(&private).unwrap();
    let database = private.join("db");
    let source = temp.path().join("file");
    fs::write(&source, b"snapshot").unwrap();
    let store = Store::open(&database).unwrap();
    store.save(&agent("a", None)).unwrap();
    let items = publish(&store, &source, &private);
    let Source::Snapshot {
        key, path: target, ..
    } = &items[0].source
    else {
        panic!("not a snapshot")
    };
    store
        .atomic(|store| {
            store.enqueue("m", "human", "a", "work")?;
            store.attach("m", &items)?;
            store.execute_cached("DELETE FROM messages WHERE id='m'", [])?;
            Ok(())
        })
        .unwrap();
    (
        Fixture {
            _temp: temp,
            database,
            source,
            private,
            key: key.clone(),
            target: target.clone(),
        },
        store,
    )
}
fn publish(store: &Store, source: &Path, private: &Path) -> Vec<Attachment> {
    let mut capture = Capture::new(store.attachments_dir.clone(), private.into()).unwrap();
    let items = capture.message(&[source.into()]).unwrap();
    capture.publish().unwrap();
    items
}
fn retry_after_reopen(fixture: &Fixture, store: Store) {
    assert_eq!(store.pending_attachments().unwrap().1.len(), 1);
    drop(store);
    let store = Store::open(&fixture.database).unwrap();
    assert_eq!(store.pending_attachments().unwrap().1.len(), 1);
    store.recover_attachments().unwrap();
    assert!(store.pending_attachments().unwrap().1.is_empty());
    assert_eq!(store.attachment_bytes().unwrap(), 0);
    assert!(!fixture.target.parent().unwrap().exists());
}

#[test]
fn every_physical_failure_retains_the_committed_outbox_for_reopen_and_retry() {
    for index in 0..8 {
        let (fixture, store) = fixture();
        let (expected, at) =
            removal_boundaries(&store.attachments_dir, &fixture.key, "file")[index].clone();
        let hits = Arc::new(AtomicUsize::new(0));
        let observed = hits.clone();
        let result = state::sync_tests::with_hook(
            move |path, step| {
                if path == expected && step == at {
                    observed.fetch_add(1, Ordering::SeqCst);
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
                Ok(())
            },
            || store.recover_attachments(),
        );
        assert!(result.is_err(), "boundary {index}");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        retry_after_reopen(&fixture, store);
    }
}

#[test]
fn a_committed_new_reference_after_failed_sync_survives_later_cleanup() {
    for index in [2, 3, 6, 7] {
        let (fixture, store) = fixture();
        let (expected, at) =
            removal_boundaries(&store.attachments_dir, &fixture.key, "file")[index].clone();
        assert!(
            state::sync_tests::with_hook(
                move |path, step| {
                    if path == expected && step == at {
                        return Err(io::Error::from_raw_os_error(libc::EIO));
                    }
                    Ok(())
                },
                || store.recover_attachments()
            )
            .is_err()
        );
        assert_eq!(store.pending_attachments().unwrap().1.len(), 1);
        let items = publish(&store, &fixture.source, &fixture.private);
        store
            .atomic(|store| {
                store.enqueue("reuse", "human", "a", "again")?;
                store.attach("reuse", &items)
            })
            .unwrap();
        assert!(store.pending_attachments().unwrap().1.is_empty());
        drop(store);
        let store = Store::open(&fixture.database).unwrap();
        store.recover_attachments().unwrap();
        assert_eq!(fs::read(&fixture.target).unwrap(), b"snapshot");
        assert_eq!(store.attachments("reuse").unwrap().len(), 1);
        store
            .atomic(|store| {
                store.execute_cached("DELETE FROM messages WHERE id='reuse'", [])?;
                Ok(())
            })
            .unwrap();
        store.recover_attachments().unwrap();
        assert!(!fixture.target.exists());
    }
}

#[test]
fn process_death_at_every_physical_boundary_leaves_retryable_outbox() {
    for index in 0..8 {
        let (fixture, store) = fixture();
        drop(store);
        let mut command = rerun_isolated(
            "runtime::store::tests::attachments::removal::crash_fixture",
            true,
        );
        command
            .env("HIVE_ATTACHMENT_DB", &fixture.database)
            .env("HIVE_ATTACHMENT_BOUNDARY", index.to_string())
            .stdout(Stdio::null());
        let mut child = ChildGuard(crate::os::spawn(&mut command).unwrap());
        let status = wait_until(Duration::from_secs(10), Duration::from_millis(5), || {
            child.try_wait().unwrap()
        });
        assert_eq!(status.code(), Some(73), "boundary {index}");
        let store = Store::open(&fixture.database).unwrap();
        retry_after_reopen(&fixture, store);
    }
}

#[test]
#[ignore = "owned outbox recovery process-death fixture"]
fn crash_fixture() {
    let database = PathBuf::from(std::env::var_os("HIVE_ATTACHMENT_DB").unwrap());
    let store = Store::open(&database).unwrap();
    let (_, pending) = store.pending_attachments().unwrap();
    assert_eq!(pending.len(), 1);
    let index: usize = std::env::var("HIVE_ATTACHMENT_BOUNDARY")
        .unwrap()
        .parse()
        .unwrap();
    let (expected, at) = removal_boundaries(
        &store.attachments_dir,
        &pending[0].key,
        &pending[0].filename,
    )[index]
        .clone();
    state::sync_tests::with_hook(
        move |path, step| {
            if path == expected && step == at {
                std::process::exit(73);
            }
            Ok(())
        },
        || store.recover_attachments(),
    )
    .unwrap();
    panic!("death boundary was not reached");
}
