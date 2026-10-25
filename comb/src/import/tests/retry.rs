use super::*;
use std::{collections::BTreeMap, error::Error as _, path::Path};

#[derive(Clone, Copy, Debug)]
enum Request {
    Import,
    Status,
    Abandon,
}
impl Request {
    fn run(self, fixture: &mut Fixture, id: &ImportId) -> Result<()> {
        match self {
            Self::Import => fixture.apply().map(|_| ()),
            Self::Status => fixture.comb.import_status().map(|_| ()),
            Self::Abandon => fixture.comb.abandon_import(id).map(|_| ()),
        }
    }
}
const REQUESTS: [Request; 3] = [Request::Import, Request::Status, Request::Abandon];
const BARRIERS: [(Operation, usize); 3] = [
    (Operation::Fsync, 1),
    (Operation::Fsync, 2),
    (Operation::FullFlush, 1),
];

#[test]
fn retained_import_barrier_errors_preserve_trusted_progress_and_native_cause() {
    for state in [
        ImportState::Pending,
        ImportState::Completed,
        ImportState::Abandoned,
    ] {
        for request in REQUESTS {
            for (operation, occurrence) in BARRIERS {
                let mut fixture = Fixture::new();
                seed(&mut fixture, state);
                let expected = fixture.status();
                let before = snapshot(fixture.root.path());
                inject(&mut fixture, operation, occurrence);
                let error = without_publication(|| request.run(&mut fixture, &expected.batch_id));
                assert_native(&error);
                assert_eq!(
                    error.import_progress.as_deref(),
                    Some(&expected),
                    "{state:?}/{request:?}/{operation:?}/{occurrence}"
                );
                assert_eq!(snapshot(fixture.root.path()), before);
                request.run(&mut fixture, &expected.batch_id).unwrap();
                let after = fixture.status();
                assert_eq!(
                    after.state,
                    match request {
                        Request::Import => ImportState::Completed,
                        Request::Abandon if state == ImportState::Pending => ImportState::Abandoned,
                        Request::Status | Request::Abandon => state,
                    }
                );
            }
        }
    }
}

#[test]
fn retry_barrier_error_does_not_invent_progress_when_a_receipt_is_corrupt() {
    for request in REQUESTS {
        for (operation, occurrence) in BARRIERS {
            let mut fixture = Fixture::new();
            let progress = fixture.pending(1);
            let receipt = fs::read_dir(fixture.root.path().join(".comb/receipts"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            fs::write(receipt, b"invalid receipt").unwrap();
            let before = snapshot(fixture.root.path());
            inject(&mut fixture, operation, occurrence);
            let error = without_publication(|| request.run(&mut fixture, &progress.batch_id));
            assert_native(&error);
            assert!(error.import_progress.is_none());
            assert_eq!(snapshot(fixture.root.path()), before);
            assert!(request.run(&mut fixture, &progress.batch_id).is_err());
        }
    }
}

fn seed(fixture: &mut Fixture, state: ImportState) {
    match state {
        ImportState::Pending => {
            fixture.pending(1);
        }
        ImportState::Completed => {
            fixture.apply().unwrap();
        }
        ImportState::Abandoned => {
            let pending = fixture.pending(1);
            fixture.comb.abandon_import(&pending.batch_id).unwrap();
        }
    }
}
fn inject(fixture: &mut Fixture, operation: Operation, occurrence: usize) {
    let recording = Arc::new(Recording::default());
    recording.barrier(Ok(()));
    recording.fail(operation, occurrence);
    fixture.comb.disk.set_durability(recording);
}
fn without_publication(call: impl FnOnce() -> Result<()>) -> Error {
    hooks::with(
        |_, step| {
            assert_ne!(
                step,
                Step::Written,
                "retry barrier failure must precede publication"
            );
            Ok(())
        },
        || call().unwrap_err(),
    )
}
fn assert_native(error: &Error) {
    let expected = Error::from(io::Error::from_raw_os_error(libc::EIO));
    assert_eq!(error.code, expected.code);
    assert_eq!(error.message, expected.message);
    assert!(error.file_committed);
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(libc::EIO)
    );
}
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.extend(snapshot(&path));
        } else {
            result.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    result
}
