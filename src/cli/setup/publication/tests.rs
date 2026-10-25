use super::*;
use crate::error::Error;
use std::{
    cell::RefCell,
    error::Error as _,
    fs, io as native,
    os::unix::fs::{MetadataExt, PermissionsExt},
    rc::Rc,
};
mod failures;
mod retry;
const OLD: &str = "auth_profiles: {}\n# original\n";
const NEW: &str = "auth_profiles: {}\n# updated\n";
type Hook = Box<dyn FnMut(&Path, Step) -> native::Result<()>>;
thread_local! { static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) }; }

pub(in crate::cli::setup) fn with<T>(
    hook: impl FnMut(&Path, Step) -> native::Result<()> + 'static,
    run: impl FnOnce() -> T,
) -> T {
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            HOOK.with(|h| h.borrow_mut().take());
        }
    }
    HOOK.with(|h| assert!(h.borrow_mut().replace(Box::new(hook)).is_none()));
    let _clear = Clear;
    run()
}
pub(super) fn checkpoint(path: &Path, step: Step) -> native::Result<()> {
    HOOK.with(|h| h.borrow_mut().as_mut().map_or(Ok(()), |h| h(path, step)))
}
pub(in crate::cli::setup) fn evidence(error: &Error) -> Option<&atomic_fs::Error> {
    error.source()?.downcast_ref()
}
fn backups(parent: &Path) -> native::Result<Vec<PathBuf>> {
    Ok(fs::read_dir(parent)?
        .map(|e| e.map(|e| e.path()))
        .collect::<native::Result<Vec<_>>>()?
        .into_iter()
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().contains(".backup-"))
        })
        .collect())
}

#[test]
fn complete_backup_is_durable_before_configuration_publication() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.yaml");
    fs::write(&path, OLD).unwrap();
    let snapshot = Snapshot::inspect(&path).unwrap();
    let target = snapshot.target.clone();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let record = seen.clone();
    let backup = with(
        move |at, step| {
            if at != target {
                if step == Step::Written {
                    assert!(!at.exists());
                }
                if step == Step::AfterDirectorySync {
                    assert_eq!(fs::read_to_string(at)?, OLD);
                    assert_eq!(fs::metadata(at)?.mode() & 0o777, 0o600);
                    record.borrow_mut().push(at.to_owned());
                }
            } else if step == Step::Written {
                assert_eq!(record.borrow().len(), 1);
                assert_eq!(fs::read_to_string(at)?, OLD);
            }
            Ok(())
        },
        || snapshot.publish(NEW),
    )
    .unwrap()
    .unwrap();
    assert_eq!(*seen.borrow(), vec![backup.clone()]);
    assert_eq!(fs::read_to_string(backup).unwrap(), OLD);
    assert_eq!(fs::read_to_string(&path).unwrap(), NEW);
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
}

#[test]
fn configuration_size_policy_retains_large_inputs_and_rejects_before_effects() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.yaml");
    let snapshot = Snapshot::inspect(&path).unwrap();
    let oversized = format!(
        "{OLD}#{}\n",
        "x".repeat(usize::try_from(config::LIMIT).unwrap())
    );
    assert!(snapshot.publish(&oversized).is_err());
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    let large = format!("{OLD}#{}\n", "x".repeat(300 * 1024));
    snapshot.publish(&large).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let before = fs::metadata(&path).unwrap();
    assert!(
        Snapshot::inspect(&path)
            .unwrap()
            .publish(&large)
            .unwrap()
            .is_none()
    );
    let after = fs::metadata(&path).unwrap();
    assert_eq!((before.ino(), before.mode()), (after.ino(), after.mode()));
    assert!(backups(temp.path()).unwrap().is_empty());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(Snapshot::inspect(&path).is_err());
}

#[test]
fn group_writable_config_parent_allows_create_replace_and_identical_resync() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.yaml");
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o775)).unwrap();
    let initial = Snapshot::inspect(&path).unwrap();
    if rustix::process::geteuid().is_root() {
        assert!(initial.publish(OLD).is_err());
        return;
    }
    assert!(initial.publish(OLD).unwrap().is_none());
    let backup = Snapshot::inspect(&path)
        .unwrap()
        .publish(NEW)
        .unwrap()
        .unwrap();
    assert_eq!(fs::read_to_string(backup).unwrap(), OLD);
    let before = fs::metadata(&path).unwrap();
    assert!(
        Snapshot::inspect(&path)
            .unwrap()
            .publish(NEW)
            .unwrap()
            .is_none()
    );
    assert_eq!(fs::metadata(&path).unwrap().ino(), before.ino());
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    assert_eq!(fs::metadata(temp.path()).unwrap().mode() & 0o777, 0o775);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o664)).unwrap();
    assert!(Snapshot::inspect(&path).is_err());
}
