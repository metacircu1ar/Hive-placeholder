//! Verify the injected durability barriers preserve state replacement and failure ordering.
use super::*;
use hive_setup_support::checkpoints::Checkpoint;
use std::cell::RefCell;

type Hook = Box<dyn Fn(&Path, Checkpoint) -> io::Result<()>>;
thread_local! { static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) }; }
struct Restore(Option<Hook>);
impl Drop for Restore {
    fn drop(&mut self) {
        HOOK.replace(self.0.take());
    }
}
pub(crate) fn with_hook<T>(
    hook: impl Fn(&Path, atomic_fs::Step) -> io::Result<()> + 'static,
    run: impl FnOnce() -> T,
) -> T {
    with_checkpoint(
        move |path, at| match at {
            Checkpoint::Publication(step) => hook(path, step),
            _ => Ok(()),
        },
        run,
    )
}
pub(crate) fn with_checkpoint<T>(
    hook: impl Fn(&Path, Checkpoint) -> io::Result<()> + 'static,
    run: impl FnOnce() -> T,
) -> T {
    let _restore = Restore(HOOK.replace(Some(Box::new(hook))));
    run()
}
pub(crate) fn step(path: &Path, at: impl Into<Checkpoint>) -> io::Result<()> {
    HOOK.with_borrow(|hook| hook.as_ref().map_or(Ok(()), |hook| hook(path, at.into())))
}
#[test]
fn injected_barriers_observe_file_before_replace_and_directory_after_replace() {
    let temp = tempfile::tempdir().expect("state directory");
    let root = temp.path().join("state");
    directory(&root).expect("private state directory");
    let path = root.join("record.json");
    write_json(&path, &serde_json::json!({"old":true})).expect("old record");
    let phases = RefCell::new(Vec::new());
    write_json_with_sync(
        &path,
        &serde_json::json!({"new":true}),
        |file, directory| {
            phases.borrow_mut().push(directory);
            let expected = if directory {
                b"{\"new\":true}\n"
            } else {
                b"{\"old\":true}\n"
            };
            assert_eq!(fs::read(&path).expect("published record"), expected);
            file.sync_all()
        },
    )
    .expect("durable replacement");
    assert_eq!(*phases.borrow(), [false, true]);
}

#[test]
fn file_sync_failure_keeps_old_record_and_directory_failure_keeps_new_record() {
    let temp = tempfile::tempdir().expect("state directory");
    let root = temp.path().join("state");
    directory(&root).expect("private state directory");
    let path = root.join("record.json");
    write_json(&path, &serde_json::json!({"old":true})).expect("old record");
    let phases = RefCell::new(Vec::new());
    let result = write_json_with_sync(&path, &serde_json::json!({"new":true}), |_, directory| {
        phases.borrow_mut().push(directory);
        Err(std::io::Error::other("file fixture"))
    });
    assert_eq!(result.expect_err("file sync failure").code(), "state_io");
    assert_eq!(*phases.borrow(), [false]);
    assert_eq!(
        fs::read(&path).expect("old record retained"),
        b"{\"old\":true}\n"
    );
    let result = write_json_with_sync(
        &path,
        &serde_json::json!({"new":true}),
        |file, directory| {
            if directory {
                Err(std::io::Error::other("directory fixture"))
            } else {
                file.sync_all()
            }
        },
    );
    assert_eq!(
        result.expect_err("directory sync failure").code(),
        "state_io"
    );
    assert_eq!(
        fs::read(&path).expect("new record retained"),
        b"{\"new\":true}\n"
    );
}

/// A record whose reader is interrupted by `replace` after each open, at most
/// `limit` times. Returns the read result and the number of opens.
fn read_while_replacing(
    limit: usize,
    replace: impl Fn(&Path) + 'static,
) -> (Result<Vec<u8>>, usize) {
    read_with(Replaced::Reopen, limit, replace)
}
fn read_with(
    replaced: Replaced,
    limit: usize,
    replace: impl Fn(&Path) + 'static,
) -> (Result<Vec<u8>>, usize) {
    let temp = tempfile::tempdir().expect("state directory");
    let root = temp.path().join("state");
    directory(&root).expect("private state directory");
    let path = root.join("record.json");
    write_json(&path, &serde_json::json!({"record":true})).expect("record");
    let opens = std::rc::Rc::new(std::cell::Cell::new(0));
    let counted = opens.clone();
    let result = with_checkpoint(
        move |at_path, at| {
            if at == Checkpoint::RecordOpened {
                counted.set(counted.get() + 1);
                if counted.get() <= limit {
                    replace(at_path);
                }
            }
            Ok(())
        },
        || read_json_bytes(&path, replaced),
    );
    (result, opens.get())
}
fn republish(path: &Path) {
    write_json(path, &serde_json::json!({"record":true})).expect("replacement");
}
#[test]
fn a_record_replaced_after_it_is_opened_is_read_again() {
    let (result, opens) = read_while_replacing(1, republish);
    assert_eq!(result.expect("replacement read"), b"{\"record\":true}\n");
    assert_eq!(opens, 2);
    let (result, opens) = read_while_replacing(usize::MAX, republish);
    assert_eq!(result.unwrap_err().code(), "unsafe_state");
    assert_eq!(opens, REPLACED_RECORD_ATTEMPTS);
    // A reader that excludes writers treats a replacement as a broken lock.
    let (result, opens) = read_with(Replaced::Refuse, 1, republish);
    assert_eq!(result.unwrap_err().code(), "unsafe_state");
    assert_eq!(opens, 1);
}
#[test]
fn replaced_records_that_are_not_private_regular_files_stay_refused() {
    // A hard link is never treated as a replacement.
    let (result, opens) = read_while_replacing(1, |path| {
        fs::hard_link(path, path.with_extension("link")).expect("hard link");
    });
    assert_eq!(result.unwrap_err().code(), "unsafe_state");
    assert_eq!(opens, 1);
    // A shared record replaced after opening is refused, not read again.
    let (result, opens) = read_while_replacing(1, |path| {
        fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o644))
            .expect("shared mode");
        let replacement = path.with_extension("next");
        fs::write(&replacement, b"{\"record\":true}\n").expect("replacement");
        fs::set_permissions(
            &replacement,
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .expect("private replacement");
        fs::rename(&replacement, path).expect("rename over the record");
    });
    assert_eq!(result.unwrap_err().code(), "unsafe_state");
    assert_eq!(opens, 1);
}
