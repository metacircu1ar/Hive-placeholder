use super::*;
use atomic_fs::{Barriers, Step};

struct Observe<'a> {
    root: &'a Path,
    fail: Step,
}
impl Barriers for Observe<'_> {
    fn observe(&self, step: Step) -> io::Result<()> {
        if step == Step::BeforePublish {
            assert!(!self.root.exists());
        }
        if step == Step::AfterPublish {
            complete(self.root);
        }
        if step == self.fail {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        Ok(())
    }
    fn sync_file(&self, _: &fs::File) -> io::Result<()> {
        panic!("disposable tree synced");
    }
    fn sync_directory(&self, _: &fs::File) -> io::Result<()> {
        panic!("disposable tree synced");
    }
}
fn complete(root: &Path) {
    assert_eq!(root.metadata().unwrap().permissions().mode() & 0o777, 0o700);
    for name in ["hive-bash", "hive-shell"] {
        let path = root.join(name);
        assert_eq!(fs::read(&path).unwrap(), b"binary");
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o500);
    }
    assert_eq!(
        fs::read(root.join("instructions")).unwrap(),
        b"instructions"
    );
    assert_eq!(
        root.join("instructions")
            .metadata()
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn control_tree_is_whole_at_publication_and_never_clobbers_a_winner() {
    use std::error::Error as _;
    for fail in [Step::BeforePublish, Step::AfterPublish] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        fs::write(&source, b"binary").unwrap();
        let root = dir.path().join("control");
        let error = prepare::publish(
            &root,
            &source,
            "instructions",
            &Observe { root: &root, fail },
        )
        .unwrap_err();
        let failure = error
            .source()
            .unwrap()
            .downcast_ref::<atomic_fs::Error>()
            .unwrap();
        assert_eq!(failure.published(), fail == Step::AfterPublish);
        assert_eq!(failure.io_error().raw_os_error(), Some(libc::EIO));
        assert_eq!(root.exists(), failure.published());
        if failure.published() {
            assert!(
                prepare::publish(&root, &source, "changed", &atomic_fs::DefaultBarriers).is_err()
            );
            complete(&root);
        }
        assert_eq!(
            fs::read_dir(dir.path()).unwrap().count(),
            1 + usize::from(failure.published())
        );
    }
}

#[test]
fn guard_directories_files_and_socket_are_private_under_permissive_umask() {
    if std::env::var_os("HIVE_GUARD_MODE_FIXTURE").is_none() {
        crate::test_support::run_isolated(crate::test_support::rerun_isolated(
            "shell::guard::tests::publication::guard_directories_files_and_socket_are_private_under_permissive_umask", false,
        ).env("HIVE_GUARD_MODE_FIXTURE", "1"));
        return;
    }
    // SAFETY: this fixture runs alone in an owned subprocess.
    unsafe {
        libc::umask(0);
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    fs::write(&source, b"binary").unwrap();
    let guard = ShellGuard::create(dir.path(), "instructions", || Ok(source)).unwrap();
    complete(&guard.directory());
    assert_eq!(
        guard
            .protected_directory()
            .metadata()
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(guard.directory().join("socket").exists());
    assert_eq!(
        guard.directory().parent(),
        Some(guard.protected_directory().as_path())
    );
}
