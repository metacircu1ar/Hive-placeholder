use super::*;
use crate::{
    ErrorCode,
    disk::{Counts, Disk, Operation},
};
use std::sync::Arc;

mod mutations;
mod page_staging;
mod publication;

fn assert_io(error: &Error, committed: bool, request: Option<&str>) {
    let expected = Error::from(io::Error::from_raw_os_error(libc::EIO));
    assert_eq!(error.code, ErrorCode::IoError);
    assert_eq!(error.message, expected.message);
    assert_eq!(error.file_committed, committed);
    assert_eq!(error.request_id.as_deref(), request);
    assert_eq!(native_cause(error).raw_os_error(), Some(libc::EIO));
}

fn native_cause(error: &Error) -> &io::Error {
    use std::error::Error as _;
    let source = error.source().unwrap();
    if let Some(publication) = source.downcast_ref::<atomic_fs::Error>() {
        publication.io_error()
    } else {
        source.downcast_ref::<io::Error>().unwrap()
    }
}

#[test]
fn replacement_failures_keep_the_rename_commit_boundary() {
    for (operation, occurrence, committed) in [
        (Operation::Fsync, 1, false),
        (Operation::Fsync, 2, true),
        (Operation::FullFlush, 1, true),
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state");
        fs::write(&path, b"before").unwrap();
        let recording = Recording::default();
        recording.barrier(Ok(()));
        recording.fail(operation, occurrence);
        let error = atomic(&path, b"after", &recording).unwrap_err();
        assert_io(&error, committed, None);
        assert_eq!(
            fs::read(&path).unwrap(),
            if committed {
                b"after".as_slice()
            } else {
                b"before"
            }
        );
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        atomic(&path, b"retry", &recording).unwrap();
        assert_eq!(fs::read(path).unwrap(), b"retry");
    }
}

#[test]
fn native_ordering_preserves_the_platform_sequence() {
    let root = tempfile::tempdir().unwrap();
    let recording = Recording::default();
    replace_ordered(&root.path().join("state"), b"value", &recording).unwrap();
    let counts: Counts = recording.counts();
    let mut expected = vec![Operation::Fsync];
    if cfg!(target_os = "macos") {
        expected.push(Operation::OrderingBarrier);
        assert!(counts.order_fallbacks <= 1);
        if counts.order_fallbacks == 1 {
            expected.push(Operation::FullFlush);
        }
    } else {
        assert_eq!(counts.order_fallbacks, 0);
    }
    expected.push(Operation::Fsync);
    assert_eq!(counts.events, expected);
    assert_eq!(counts.full_barriers, counts.order_fallbacks);
}

fn injected_order(file: &File, recording: &Recording) -> Result<()> {
    recording.fsync(file)?;
    barrier_after_sync(file, recording).map_err(Error::from)
}

#[test]
fn supported_and_unsupported_ordering_have_exact_fallback_events() {
    let file = tempfile::tempfile().unwrap();
    for errno in [
        None,
        Some(libc::EINVAL),
        Some(libc::ENOTSUP),
        Some(libc::ENOSYS),
    ] {
        let recording = Recording::default();
        recording.barrier(errno.map_or(Ok(()), Err));
        injected_order(&file, &recording).unwrap();
        let mut expected = vec![Operation::Fsync, Operation::OrderingBarrier];
        if errno.is_some() {
            expected.push(Operation::FullFlush);
        }
        let counts = recording.counts();
        assert_eq!(counts.events, expected);
        assert_eq!(counts.full_barriers, usize::from(errno.is_some()));
        assert_eq!(counts.order_fallbacks, usize::from(errno.is_some()));
    }
}

#[test]
fn ordering_failures_never_turn_into_unrequested_fallbacks() {
    let file = tempfile::tempfile().unwrap();
    let recording = Recording::default();
    recording.barrier(Err(libc::EIO));
    assert_io(&injected_order(&file, &recording).unwrap_err(), false, None);
    assert_eq!(recording.counts().full_barriers, 0);
    assert_eq!(recording.counts().order_fallbacks, 0);
    assert_eq!(
        recording.counts().events,
        [Operation::Fsync, Operation::OrderingBarrier]
    );

    recording.reset();
    recording.barrier(Err(libc::ENOTSUP));
    recording.fail(Operation::FullFlush, 1);
    assert_io(&injected_order(&file, &recording).unwrap_err(), false, None);
    let counts = recording.counts();
    assert_eq!(counts.full_barriers, 1);
    assert_eq!(counts.order_fallbacks, 1);
    assert_eq!(
        counts.events,
        [
            Operation::Fsync,
            Operation::OrderingBarrier,
            Operation::FullFlush
        ]
    );
}

#[cfg(target_os = "macos")]
#[test]
fn an_ordering_failure_preserves_the_existing_destination() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state");
    fs::write(&path, b"before").unwrap();
    let recording = Recording::default();
    recording.barrier(Err(libc::EIO));
    assert_io(
        &atomic(&path, b"after", &recording).unwrap_err(),
        false,
        None,
    );
    assert_eq!(fs::read(&path).unwrap(), b"before");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    recording.barrier(Ok(()));
    atomic(&path, b"retry", &recording).unwrap();
    assert_eq!(fs::read(path).unwrap(), b"retry");
}

#[test]
fn acquisition_counts_directory_creation_separately() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("new/nested");
    let recording = Arc::new(Recording::default());
    recording.barrier(Ok(()));
    let disk = Disk::acquire_with(&root, true, recording.clone()).unwrap();
    let counts = recording.counts();
    assert_eq!(
        counts
            .events
            .iter()
            .filter(|&&op| op == Operation::DirectorySync)
            .count(),
        10
    );
    // .gitignore, lock, identity.json, and the current format marker.
    assert_eq!(counts.full_barriers, 4);
    assert_eq!(counts.order_fallbacks, 0);
    assert!(Arc::ptr_eq(
        &disk.durability(),
        &(recording.clone() as Arc<dyn Durability>)
    ));
}

#[test]
fn owners_keep_counters_and_failures_separate_on_one_thread() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let a = Arc::new(Recording::default());
    let b = Arc::new(Recording::default());
    let a_disk = Disk::acquire_with(first.path(), true, a.clone()).unwrap();
    let b_disk = Disk::acquire_with(second.path(), true, b.clone()).unwrap();
    a.reset();
    b.reset();
    a.fail_finish_once();
    b_disk.index_population(true).unwrap();
    b_disk.finish_population().unwrap();
    assert!(a.counts().events.is_empty());
    let before = b.counts().events;
    let error = a_disk.finish().unwrap_err();
    assert_eq!(error.code, ErrorCode::IoError);
    assert_eq!(error.message, "Injected finalization failure.");
    assert!(error.file_committed);
    assert!(error.request_id.is_none());
    assert_eq!(a.counts().events, [Operation::BeforeFinish]);
    assert_eq!(b.counts().events, before);
    a_disk.index_population(true).unwrap();
    a_disk.finish_population().unwrap();
}

#[test]
fn population_completion_does_not_consume_the_finish_checkpoint() {
    let root = tempfile::tempdir().unwrap();
    let recording = Arc::new(Recording::default());
    let disk = Disk::acquire_with(root.path(), true, recording.clone()).unwrap();
    disk.index_population(true).unwrap();
    recording.reset();
    recording.fail_finish_once();
    disk.finish_population().unwrap();
    assert_eq!(
        recording.counts().events,
        [Operation::FullFlush, Operation::Fsync]
    );
    assert_eq!(
        disk.finish().unwrap_err().message,
        "Injected finalization failure."
    );
    assert_eq!(
        recording.counts().events.last(),
        Some(&Operation::BeforeFinish)
    );
    assert!(!disk.admin.join("intent.json").exists());
}
