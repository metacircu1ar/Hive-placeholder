use super::*;
use crate::runtime::{store::Store, test_support::AgentBuilder};
use std::{cell::RefCell, path::PathBuf, rc::Rc};
mod process;
mod reader;

const IDS: [&str; 2] = [
    "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa",
    "bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb",
];
const PEER: &str = "cccccccc-cccc-4ccc-cccc-cccccccccccc";
const STEPS: [Step; 10] = [
    Step::BeforeRemove,
    Step::AfterRemove,
    Step::BeforeDirectorySync,
    Step::AfterDirectorySync,
    Step::BeforeRemove,
    Step::AfterRemove,
    Step::BeforeRemove,
    Step::AfterRemove,
    Step::BeforeDirectorySync,
    Step::AfterDirectorySync,
];
type Hook = Box<dyn Fn(Step) -> io::Result<()>>;
thread_local! { static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) }; }
pub(super) fn step(value: Step) -> io::Result<()> {
    HOOK.with_borrow(|hook| hook.as_ref().map_or(Ok(()), |hook| hook(value)))
}
struct Restore(Option<Hook>);
impl Drop for Restore {
    fn drop(&mut self) {
        HOOK.replace(self.0.take());
    }
}
fn with_hook<T>(hook: impl Fn(Step) -> io::Result<()> + 'static, run: impl FnOnce() -> T) -> T {
    let _restore = Restore(HOOK.replace(Some(Box::new(hook))));
    run()
}
fn seed(root: &Path) -> Store {
    let store = Store::open(&root.join("runtime.sqlite")).unwrap();
    store.save(&AgentBuilder::new("peer").build()).unwrap();
    for id in IDS.into_iter().chain([PEER]) {
        state::write_json(&output(root, id), &serde_json::json!({"keep":true})).unwrap();
    }
    for id in IDS {
        store
            .connection
            .execute("INSERT INTO artifact_deletions VALUES(?1)", [id])
            .unwrap();
    }
    store
        .connection
        .execute(
            "INSERT INTO artifacts(id,bytes,created_at,agent) VALUES(?1,14,0,'peer')",
            [PEER],
        )
        .unwrap();
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .create_new(manifest(root), b"derived manifest\n")
        .unwrap();
    store
}
fn output(root: &Path, id: &str) -> PathBuf {
    root.join("output").join(format!("{id}.json"))
}
fn manifest(root: &Path) -> PathBuf {
    root.join("output-manifest.jsonl")
}
fn peer_unchanged(store: &Store) {
    assert_eq!(
        store.output(PEER, 0, 100).unwrap()["text"],
        "{\"keep\":true}\n"
    );
}
fn interrupted(root: &Path, at: usize) {
    assert_eq!(manifest(root).exists(), at == 0);
    assert_eq!(output(root, IDS[0]).exists(), at < 5);
    assert_eq!(output(root, IDS[1]).exists(), at < 7);
    let reopened = Store::open(&root.join("runtime.sqlite")).unwrap();
    assert_eq!(reopened.pending_artifact_deletions().unwrap().ids, IDS);
    peer_unchanged(&reopened);
    reopened.finish_artifact_deletions().unwrap();
    assert!(
        reopened
            .pending_artifact_deletions()
            .unwrap()
            .ids
            .is_empty()
    );
    assert!(!manifest(root).exists());
    for id in IDS {
        assert!(!output(root, id).exists());
    }
    peer_unchanged(&reopened);
}

#[test]
fn every_unlink_and_sync_failure_retains_the_outbox_and_restarts_safely() {
    for at in 0..STEPS.len() {
        let temporary = crate::runtime::test_directory();
        let store = seed(temporary.path());
        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = events.clone();
        let error = with_hook(
            move |step| {
                observed.borrow_mut().push(step);
                if observed.borrow().len() == at + 1 {
                    Err(io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
            || store.finish_artifact_deletions(),
        )
        .unwrap_err();
        assert_eq!(error.code(), "state_io");
        assert_eq!(error.io_raw_os_error(), Some(libc::EIO));
        assert_eq!(events.borrow().as_slice(), &STEPS[..=at]);
        drop(store);
        interrupted(temporary.path(), at);
    }
}

#[test]
fn absent_manifest_retry_repeats_its_sync_before_deleting_any_output() {
    let temporary = crate::runtime::test_directory();
    let store = seed(temporary.path());
    for _ in 0..2 {
        let error = with_hook(
            |step| {
                if step == Step::BeforeDirectorySync {
                    Err(io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
            || store.finish_artifact_deletions(),
        )
        .unwrap_err();
        assert_eq!(error.io_raw_os_error(), Some(libc::EIO));
        assert!(!manifest(temporary.path()).exists());
        for id in IDS {
            assert!(output(temporary.path(), id).exists());
        }
        assert_eq!(store.pending_artifact_deletions().unwrap().ids, IDS);
        peer_unchanged(&store);
    }
    store.finish_artifact_deletions().unwrap();
    assert!(store.pending_artifact_deletions().unwrap().ids.is_empty());
}

#[test]
fn malformed_or_noncanonical_outbox_ids_do_not_invalidate_or_delete_anything() {
    for alias in [
        PEER.to_uppercase(),
        PEER.replace('-', ""),
        format!("{{{PEER}}}"),
        "../peer".into(),
    ] {
        let temporary = crate::runtime::test_directory();
        let store = seed(temporary.path());
        store
            .connection
            .execute("INSERT INTO artifact_deletions VALUES(?1)", [&alias])
            .unwrap();
        let before = store.pending_artifact_deletions().unwrap().ids;
        assert_eq!(
            store.finish_artifact_deletions().unwrap_err().code(),
            "database_error"
        );
        assert_eq!(store.pending_artifact_deletions().unwrap().ids, before);
        assert_eq!(
            fs::read(manifest(temporary.path())).unwrap(),
            b"derived manifest\n"
        );
        for id in IDS {
            assert!(output(temporary.path(), id).exists());
        }
        peer_unchanged(&store);
    }
}

#[test]
fn unsafe_manifest_and_output_preserve_peers_and_leave_the_outbox_retryable() {
    use std::os::unix::fs::symlink;
    for is_manifest in [true, false] {
        let temporary = crate::runtime::test_directory();
        let root = temporary.path();
        let store = seed(root);
        let path = if is_manifest {
            manifest(root)
        } else {
            output(root, IDS[0])
        };
        fs::remove_file(&path).unwrap();
        symlink(output(root, PEER), &path).unwrap();
        assert_eq!(
            store.finish_artifact_deletions().unwrap_err().code(),
            "unsafe_state"
        );
        assert!(path.symlink_metadata().unwrap().is_symlink());
        assert!(output(root, IDS[1]).exists());
        if is_manifest {
            assert!(output(root, IDS[0]).exists());
        }
        peer_unchanged(&store);
        assert_eq!(store.pending_artifact_deletions().unwrap().ids, IDS);
        fs::remove_file(path).unwrap();
        store.finish_artifact_deletions().unwrap();
        assert!(store.pending_artifact_deletions().unwrap().ids.is_empty());
        peer_unchanged(&store);
    }
}
