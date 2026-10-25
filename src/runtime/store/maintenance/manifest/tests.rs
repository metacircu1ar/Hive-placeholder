use super::*;
use crate::runtime::test_support::AgentBuilder;
use atomic_fs::Step;
use std::{error::Error as _, os::unix::fs::MetadataExt, path::PathBuf};

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
const NEW: &[u8] = b"{\"bytes\":14,\"created_at\":7,\"id\":\"a\",\"path\":\"output/a.json\"}\n{\"bytes\":14,\"created_at\":7,\"id\":\"b\",\"path\":\"output/b.json\"}\n";

fn fixture() -> (tempfile::TempDir, Store) {
    let temp = crate::runtime::test_directory();
    drop(
        crate::state::State::new(temp.path().into())
            .unwrap()
            .initialize()
            .unwrap(),
    );
    let runtime = temp.path().join("runtime");
    state::directory(&runtime).unwrap();
    let store = Store::open(&runtime.join("runtime.sqlite")).unwrap();
    store.save(&AgentBuilder::new("peer").build()).unwrap();
    for id in ["b", "a"] {
        store
            .connection
            .execute(
                "INSERT INTO artifacts(id,bytes,created_at,agent) VALUES(?1,14,7,'peer')",
                [id],
            )
            .unwrap();
    }
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(target(&runtime), OLD)
        .unwrap();
    (temp, store)
}

fn target(runtime: &Path) -> PathBuf {
    runtime.join("output-manifest.jsonl")
}
fn stages(runtime: &Path) -> Vec<PathBuf> {
    fs::read_dir(runtime)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .is_some_and(hive_setup_support::names::state_preparation_name)
        })
        .collect()
}
fn publication(error: &Error) -> &atomic_fs::Error {
    error
        .source()
        .unwrap()
        .downcast_ref::<atomic_fs::Error>()
        .unwrap()
}

#[test]
fn every_manifest_fault_keeps_complete_ordered_rows_and_exact_commit_evidence() {
    let _fork = crate::os::fork_guard().unwrap();
    for (index, at) in STEPS.into_iter().enumerate() {
        let (temp, store) = fixture();
        let runtime = temp.path().join("runtime");
        let expected = target(&runtime);
        let error = state::sync_tests::with_hook(
            move |path, step| {
                if path == expected && step == at {
                    Err(io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
            || store.manifest(&runtime),
        )
        .unwrap_err();
        assert_eq!(error.code(), "state_io");
        assert_eq!(
            error.io_kind(),
            Some(io::Error::from_raw_os_error(libc::EIO).kind())
        );
        assert_eq!(error.io_raw_os_error(), Some(libc::EIO));
        let failure = publication(&error);
        assert_eq!(failure.published(), index >= 4);
        assert!(failure.cleanup_error().is_none() && failure.retained().is_none());
        assert_eq!(
            fs::read(target(&runtime)).unwrap(),
            if index >= 4 { NEW } else { OLD }
        );
        assert!(stages(&runtime).is_empty());
        store.manifest(&runtime).unwrap();
        assert_eq!(fs::read(target(&runtime)).unwrap(), NEW);
    }
}

#[test]
fn partial_sql_failure_leaves_the_old_manifest_and_retains_database_cause() {
    let (temp, store) = fixture();
    let runtime = temp.path().join("runtime");
    store
        .connection
        .execute("UPDATE artifacts SET bytes='invalid' WHERE id='b'", [])
        .unwrap();
    let error = store.manifest(&runtime).unwrap_err();
    assert_eq!(error.code(), "database_error");
    assert_eq!(error.io_kind(), None);
    let failure = publication(&error);
    let public = failure
        .io_error()
        .get_ref()
        .unwrap()
        .downcast_ref::<Error>()
        .unwrap();
    assert!(public.source().unwrap().is::<rusqlite::Error>());
    assert!(
        !failure.published() && failure.cleanup_error().is_none() && failure.retained().is_none()
    );
    assert_eq!(fs::read(target(&runtime)).unwrap(), OLD);
    assert!(stages(&runtime).is_empty());
}

#[test]
fn empty_manifest_is_private_and_unsafe_targets_remain_untouched() {
    let (temp, store) = fixture();
    let runtime = temp.path().join("runtime");
    store
        .connection
        .execute("DELETE FROM artifacts", [])
        .unwrap();
    store.manifest(&runtime).unwrap();
    assert!(fs::read(target(&runtime)).unwrap().is_empty());
    assert_eq!(
        fs::metadata(target(&runtime)).unwrap().mode() & 0o777,
        0o600
    );
    fs::remove_file(target(&runtime)).unwrap();
    state::directory(&target(&runtime)).unwrap();
    assert_eq!(store.manifest(&runtime).unwrap_err().code(), "unsafe_state");
    fs::remove_dir(target(&runtime)).unwrap();
    let sentinel = runtime.join("sentinel");
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .write(&sentinel, OLD)
        .unwrap();
    std::os::unix::fs::symlink(&sentinel, target(&runtime)).unwrap();
    assert_eq!(store.manifest(&runtime).unwrap_err().code(), "unsafe_state");
    assert_eq!(fs::read(sentinel).unwrap(), OLD);
    assert!(stages(&runtime).is_empty());
}

#[test]
fn wrapped_write_error_keeps_native_classification_and_all_cleanup_evidence() {
    for published in [false, true] {
        let cause = Error::state_io_from(io::Error::from_raw_os_error(libc::ENOSPC));
        let error = Error::manifest_publication(
            atomic_fs::Error::new(io::Error::other(cause), published)
                .with_cleanup_error(io::Error::from_raw_os_error(libc::EACCES))
                .with_retained(PathBuf::from("retained-preparation")),
        );
        assert_eq!(error.code(), "state_io");
        assert_eq!(
            error.io_kind(),
            Some(io::Error::from_raw_os_error(libc::ENOSPC).kind())
        );
        assert_eq!(error.io_raw_os_error(), Some(libc::ENOSPC));
        let failure = publication(&error);
        assert_eq!(failure.published(), published);
        assert_eq!(
            failure.cleanup_error().as_ref().unwrap().raw_os_error(),
            Some(libc::EACCES)
        );
        assert_eq!(failure.retained(), Some(Path::new("retained-preparation")));
    }
}

#[test]
fn actual_row_writer_retains_io_failure_after_partial_output() {
    struct Full(Vec<u8>);
    impl Write for Full {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0.len() >= 20 {
                return Err(io::Error::from_raw_os_error(libc::ENOSPC));
            }
            let count = bytes.len().min(20 - self.0.len());
            self.0.extend_from_slice(&bytes[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let (_temp, store) = fixture();
    let mut writer = Full(Vec::new());
    let error = store.write_manifest(&mut writer).unwrap_err();
    assert_eq!(writer.0.len(), 20);
    assert_eq!(error.code(), "state_io");
    assert_eq!(error.io_raw_os_error(), Some(libc::ENOSPC));
}

mod process;
