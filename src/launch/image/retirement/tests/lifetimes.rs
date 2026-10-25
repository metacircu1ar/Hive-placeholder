use super::*;
use crate::{
    launch::LauncherCache,
    test_support::{ChildGuard, rerun_isolated, wait_until},
};
use std::{
    process::{Command, Stdio},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

#[test]
fn drop_reclaims_with_a_separately_owned_root_and_never_waits_for_cache_lock() {
    let _fork = crate::os::fork_guard().unwrap();
    for busy in [false, true] {
        let root = private_root().unwrap();
        let image = fixture(root.path());
        let retirement = image.retirement.clone();
        let lock = busy.then(|| state::Lock::index(&root.path().join("cache.lock")).unwrap());
        let start = Instant::now();
        drop(image);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(root.path().is_dir());
        assert_eq!(retirement.original.exists(), busy);
        assert!(!retirement.aside.exists());
        drop(lock);
        sweep(root.path());
        assert!(!retirement.original.exists());
    }
}

#[test]
fn unwind_and_ambiguous_helper_failure_retain_evidence_without_implicit_deletion() {
    let _fork = crate::os::fork_guard().unwrap();
    let root = private_root().unwrap();
    fs::write(root.path().join("source"), b"launcher").unwrap();
    let result = std::panic::catch_unwind(|| {
        build_image(root.path(), &root.path().join("source"), |_| {
            panic!("fixture unwind")
        })
    });
    assert!(result.is_err());
    let abandoned = fs::read_dir(root.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    assert!(abandoned.join("owner.lock").exists());
    sweep(root.path());
    assert!(!abandoned.exists());
    let mut image = fixture(root.path());
    let original = image.directory.clone();
    let error = crate::publication::error(
        atomic_fs::Error::new(io::Error::other("ambiguous fixture"), false)
            .with_retained(image.path.clone()),
    );
    image.failed(&error);
    drop(image);
    assert!(original.join("hive-launch").exists());
    sweep(root.path());
    assert!(!original.exists());
}

#[test]
fn preowner_cleanup_only_removes_an_identity_checked_empty_reservation() {
    let _fork = crate::os::fork_guard().unwrap();
    let root = private_root().unwrap();
    let _cache = state::Lock::index(&root.path().join("cache.lock")).unwrap();
    for nonempty in [true, false] {
        let retirement = Retirement::allocate(
            root.path(),
            if nonempty {
                "hive-launch-full"
            } else {
                "hive-launch-empty"
            },
        )
        .unwrap();
        if nonempty {
            state::private_file(&retirement.original.join("sentinel"), true).unwrap();
        }
        assert_eq!(retirement.remove_empty_reservation().is_err(), nonempty);
        assert_eq!(retirement.original.exists(), nonempty);
    }
}

#[test]
fn lookup_through_spawn_holds_the_cache_while_another_thread_sweeps() {
    let root = private_root().unwrap();
    let source_cache = crate::launch::tests::test_cache().unwrap();
    let executable = source_cache.executable().unwrap();
    let location = root.path().to_owned();
    let cache = Arc::new(LauncherCache::new(
        move || Ok(location.clone()),
        move || Ok(executable.clone()),
    ));
    let held = cache.clone();
    let (ready, arrived) = mpsc::channel();
    let (release, go) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        state::sync_tests::with_hook(
            move |path, step| {
                if path.file_name().is_some_and(|name| name == "hive-launch")
                    && step == Step::BeforePublish
                {
                    ready.send(path.to_owned()).unwrap();
                    go.recv_timeout(Duration::from_secs(10)).unwrap();
                }
                Ok(())
            },
            || {
                let mut child = ChildGuard(
                    crate::launch::spawn(&held, &Command::new("/usr/bin/true"), true, &[], |_| {
                        Ok(())
                    })
                    .unwrap(),
                );
                assert!(child.wait().unwrap().success());
            },
        );
    });
    let path = arrived.recv_timeout(Duration::from_secs(10)).unwrap();
    drop(cache);
    sweep(root.path());
    let preserved = path.exists();
    release.send(()).unwrap();
    worker.join().unwrap();
    assert!(preserved);
    assert!(!path.parent().unwrap().exists());
}

#[test]
fn system_image_survives_exit_and_the_next_command_reuses_it() {
    // A private, owned fixture HOME outside shared temporary roots exercises the
    // real SYSTEM cache without touching the user's launcher directory.
    let home = tempfile::Builder::new()
        .prefix(".hive-exit-fixture-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(std::env::var_os("HOME").unwrap())
        .unwrap();
    let mut previous: Option<(PathBuf, u64)> = None;
    for _ in 0..2 {
        let mut command = rerun_isolated(
            "launch::image::retirement::tests::lifetimes::system_exit_fixture",
            true,
        );
        command
            .env("HOME", home.path())
            .env("HIVE_EXIT_PATH", home.path().join("selected"))
            .stdout(Stdio::null());
        let mut child = ChildGuard(crate::os::spawn(&mut command).unwrap());
        let status = wait_until(Duration::from_secs(10), Duration::from_millis(5), || {
            child.try_wait().unwrap()
        });
        assert_eq!(status.code(), Some(73));
        let path = PathBuf::from(fs::read_to_string(home.path().join("selected")).unwrap());
        assert!(path.is_file(), "an exit callback removed the SYSTEM image");
        let inode = std::os::unix::fs::MetadataExt::ino(&fs::metadata(&path).unwrap());
        if let Some(old) = &previous {
            // The same executable runs its earlier, already-assessed image.
            assert_eq!(*old, (path.clone(), inode));
        }
        previous = Some((path, inode));
    }
    // A sweep that keeps no content name reclaims the unheld image.
    sweep(&home.path().join(".hive-launch-images"));
    assert!(!previous.unwrap().0.exists());
}

#[test]
#[ignore = "owned SYSTEM-cache process-exit fixture"]
fn system_exit_fixture() {
    let image = crate::launch::system_cache().image().unwrap();
    fs::write(
        PathBuf::from(std::env::var_os("HIVE_EXIT_PATH").unwrap()),
        image.to_str().unwrap(),
    )
    .unwrap();
    std::process::exit(73);
}
