use super::*;
use crate::cli::setup::skills::owned::Group;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::PathBuf,
};
mod failures;
mod process;
mod retry;

type Hook = Box<dyn FnMut(&Path, Checkpoint) -> stdio::Result<()>>;
thread_local! {
    static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
}
pub(super) fn checkpoint(path: &Path, step: Checkpoint) -> stdio::Result<()> {
    HOOK.with(|hook| match hook.borrow_mut().as_mut() {
        Some(hook) => hook(path, step),
        None => Ok(()),
    })
}
pub(in crate::cli::setup::skills) struct Hooks;
impl Hooks {
    pub(in crate::cli::setup::skills) fn new(
        mut hook: impl FnMut(&Path, Step) -> stdio::Result<()> + 'static,
    ) -> Self {
        Self::at_checkpoint(move |path, at| match at {
            Checkpoint::Publication(step) => hook(path, step),
            _ => Ok(()),
        })
    }
    fn at_checkpoint(hook: impl FnMut(&Path, Checkpoint) -> stdio::Result<()> + 'static) -> Self {
        HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
        Self
    }
}
impl Drop for Hooks {
    fn drop(&mut self) {
        HOOK.with(|slot| *slot.borrow_mut() = None);
    }
}
fn fixture() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("skill");
    (temp, root)
}
fn group(root: &Path, guide: &[u8], doc: Option<&[u8]>) -> Group {
    let mut desired = BTreeMap::from([(PathBuf::from("SKILL.md"), guide.to_vec())]);
    if let Some(doc) = doc {
        desired.insert(PathBuf::from("bundle/nested/doc.md"), doc.to_vec());
    }
    Group::inspect(root.to_owned(), desired).unwrap()
}
fn native(error: &Error) -> &atomic_fs::Error {
    use std::error::Error as _;
    error.source().unwrap().downcast_ref().unwrap()
}
fn errno() -> stdio::Error {
    stdio::Error::from_raw_os_error(libc::EIO)
}
fn fail(path: PathBuf, at: impl Into<Checkpoint>) -> Hooks {
    let at = at.into();
    Hooks::at_checkpoint(move |candidate, step| {
        if candidate == path && step == at {
            return Err(errno());
        }
        Ok(())
    })
}

#[test]
fn complete_publication_preserves_old_bytes_until_commit_and_new_mode() {
    let (_temp, root) = fixture();
    group(&root, b"old", None).apply().unwrap();
    let target = root.join("SKILL.md");
    let checked = target.clone();
    let hooks = Hooks::new(move |path, step| {
        if path == checked && step == Step::BeforePublish {
            assert_eq!(fs::read(path).unwrap(), b"old");
        }
        if path == checked && step == Step::AfterPublish {
            assert_eq!(fs::read(path).unwrap(), b"new");
            assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o600);
        }
        Ok(())
    });
    group(&root, b"new", None).apply().unwrap();
    drop(hooks);
    assert_eq!(fs::read(target).unwrap(), b"new");
}

#[test]
fn rejects_desired_bounds_and_paths_before_effects() {
    let (_temp, root) = fixture();
    let too_large = vec![0; 2 * 1024 * 1024 + 1];
    assert!(
        Group::inspect(
            root.clone(),
            BTreeMap::from([(PathBuf::from("SKILL.md"), too_large)])
        )
        .is_err()
    );
    assert!(!root.exists());
    let too_many = (0..2049)
        .map(|i| (PathBuf::from(format!("bundle/{i}.md")), vec![]))
        .collect();
    assert!(Group::inspect(root.clone(), too_many).is_err());
    assert!(
        Group::inspect(
            root.clone(),
            BTreeMap::from([(PathBuf::from("bundle/../outside"), vec![])])
        )
        .is_err()
    );
    assert!(!root.exists());
}

#[test]
fn identical_unowned_file_is_adopted_with_barriers_and_preserved_mode() {
    let (_temp, root) = fixture();
    fs::create_dir(&root).unwrap();
    let path = root.join("SKILL.md");
    fs::write(&path, b"same").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let before = fs::metadata(&path).unwrap();
    let hooks = fail(path.clone(), Step::BeforeFileSync);
    assert!(group(&root, b"same", None).apply().is_err());
    drop(hooks);
    group(&root, b"same", None).apply().unwrap();
    let after = fs::metadata(path).unwrap();
    assert_eq!((after.ino(), after.mode()), (before.ino(), before.mode()));
}

#[test]
fn large_skill_uses_file_bound_and_refuses_unsafe_existing_mode() {
    let (_temp, root) = fixture();
    let bytes = vec![b'x'; 300 * 1024];
    group(&root, &bytes, None).apply().unwrap();
    let path = root.join("SKILL.md");
    assert_eq!(fs::read(&path).unwrap(), bytes);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(
        Group::inspect(
            root.clone(),
            BTreeMap::from([(PathBuf::from("SKILL.md"), bytes)])
        )
        .is_err()
    );
    assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o666);
}
