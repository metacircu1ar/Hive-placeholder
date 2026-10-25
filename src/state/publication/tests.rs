use super::*;
use crate::state;
use atomic_fs::Step;
use serde_json::json;
use std::{
    error::Error as _,
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::{Arc, Mutex},
};

const STEPS: [Step; 7] = [
    Step::Written,
    Step::BeforeFileSync,
    Step::AfterFileSync,
    Step::BeforePublish,
    Step::AfterPublish,
    Step::BeforeDirectorySync,
    Step::AfterDirectorySync,
];
const OLD: &[u8] = b"{\"old\":true}\n";
const NEW: &[u8] = b"{\"new\":true}\n";

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let temp = crate::runtime::test_directory();
    let root = temp.path().join("state");
    state::directory(&root).unwrap();
    let path = root.join("record.json");
    write_json(&path, &json!({"old":true})).unwrap();
    (temp, path)
}

#[test]
fn every_fault_preserves_the_commit_boundary_errno_and_single_sync_sequence() {
    verify_faults(write_json);
}

#[test]
fn provider_ledger_callback_keeps_all_fault_boundaries() {
    use crate::observability::state_sync::{self, LedgerPhase};
    verify_faults(|path, value| state_sync::write_json(path, value, LedgerPhase::Finish));
}

fn verify_faults(write: impl Fn(&Path, &serde_json::Value) -> Result<()>) {
    for (index, at) in STEPS.into_iter().enumerate() {
        let (_temp, path) = fixture();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let calls = observed.clone();
        let target = path.clone();
        let error = state::sync_tests::with_hook(
            move |path, step| {
                if path == target {
                    calls.lock().unwrap().push(step);
                    if step == at {
                        return Err(io::Error::from_raw_os_error(libc::EIO));
                    }
                }
                Ok(())
            },
            || write(&path, &json!({"new":true})),
        )
        .unwrap_err();
        assert_eq!(*observed.lock().unwrap(), STEPS[..=index]);
        assert_eq!(error.code(), "state_io");
        assert_eq!(error.io_raw_os_error(), Some(libc::EIO));
        let failure = error
            .source()
            .unwrap()
            .downcast_ref::<atomic_fs::Error>()
            .unwrap();
        assert_eq!(failure.published(), index >= 4);
        assert!(failure.cleanup_error().is_none() && failure.retained().is_none());
        assert_eq!(fs::read(&path).unwrap(), if index >= 4 { NEW } else { OLD });
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }
}

#[test]
fn serialization_failure_never_creates_a_preparation_or_changes_the_target() {
    struct Failing;
    impl Serialize for Failing {
        fn serialize<S: serde::Serializer>(&self, _: S) -> std::result::Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("fixture serialization failure"))
        }
    }
    let (_temp, path) = fixture();
    let error = write_json(&path, &Failing).unwrap_err();
    assert_eq!(error.code(), "state_inconsistent");
    assert!(error.source().unwrap().is::<serde_json::Error>());
    assert_eq!(fs::read(&path).unwrap(), OLD);
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn readers_see_complete_json_and_newline_during_replacement() {
    use std::{cell::Cell, sync::mpsc, time::Duration};
    let (_temp, path) = fixture();
    let values = [
        json!({"a":"a".repeat(131_072)}),
        json!({"b":"b".repeat(131_072)}),
    ];
    let encoded = values.each_ref().map(|value| {
        let mut bytes = serde_json::to_vec(value).unwrap();
        bytes.push(b'\n');
        bytes
    });
    write_json(&path, &values[0]).unwrap();
    let (request, requests) = mpsc::channel();
    let (reply, replies) = mpsc::channel();
    let target = path.clone();
    let current = Cell::new(0);
    let timeout = Duration::from_secs(10);
    std::thread::scope(|scope| {
        let read_path = &path;
        let read_values = &encoded;
        scope.spawn(move || {
            for _ in 0..128 {
                let expected: usize = requests.recv_timeout(timeout).unwrap();
                let bytes = fs::read(read_path).unwrap();
                assert_eq!(bytes, read_values[expected]);
                reply.send(()).unwrap();
            }
        });
        state::sync_tests::with_hook(
            move |path, step| {
                if path == target && matches!(step, Step::BeforePublish | Step::AfterPublish) {
                    if step == Step::AfterPublish {
                        current.set(1 - current.get());
                    }
                    request.send(current.get()).unwrap();
                    replies.recv_timeout(timeout).unwrap();
                }
                Ok(())
            },
            || {
                for index in 0..64 {
                    write_json(&path, &values[(index + 1) % 2]).unwrap();
                }
            },
        );
    });
    assert_eq!(fs::read(&path).unwrap(), encoded[0]);
    assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o600);
}

mod process;
mod staging;
