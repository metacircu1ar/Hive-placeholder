use super::*;
use crate::test_support::{ChildGuard, rerun_isolated, wait_until};
use std::{cell::RefCell, process::Stdio, time::Duration};

type Hook = Box<dyn Fn(&Path, &mut dyn io::Write) -> io::Result<()>>;
thread_local! { static HOOK: RefCell<Option<Hook>> = RefCell::new(None); }
pub(super) fn boundary(path: &Path, output: &mut dyn io::Write) -> io::Result<()> {
    HOOK.with(|hook| {
        hook.borrow()
            .as_ref()
            .map_or(Ok(()), |hook| hook(path, output))
    })
}

#[test]
fn killed_builder_leaves_a_private_reservation_reclaimed_by_the_next_sweep() {
    let root = crate::launch::tests::private_root().unwrap();
    let source = root.path().join("source");
    fs::write(&source, b"binary").unwrap();
    let mut command = rerun_isolated("launch::image::copy_tests::crash_fixture", true);
    command
        .env("HIVE_COPY_CRASH_ROOT", root.path())
        .stdout(Stdio::null());
    let mut child = ChildGuard(crate::os::spawn(&mut command).unwrap());
    let status = wait_until(Duration::from_secs(10), Duration::from_millis(5), || {
        child.try_wait().unwrap()
    });
    assert_eq!(status.code(), Some(73));
    let directory = fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.is_dir())
        .unwrap();
    assert!(!directory.join("hive-launch").exists());
    let reservation = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.file_name().unwrap() != "owner.lock")
        .unwrap();
    assert_eq!(fs::read(&reservation).unwrap(), b"partial");
    assert_eq!(
        reservation.metadata().unwrap().permissions().mode() & 0o777,
        0o500
    );
    let _cache = state::Lock::index(&root.path().join("cache.lock")).unwrap();
    sweep_images(root.path(), None);
    assert!(!directory.exists());
}

#[test]
#[ignore = "owned subprocess fixture; exercised by killed_builder_leaves_a_private_reservation_reclaimed_by_the_next_sweep"]
fn crash_fixture() {
    let root = PathBuf::from(std::env::var_os("HIVE_COPY_CRASH_ROOT").unwrap());
    HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(|_, output| {
            output.write_all(b"partial")?;
            // The publication producer is buffered; reach the disk-write crash
            // boundary explicitly so the sweep still exercises a partial file.
            output.flush()?;
            std::process::exit(73);
        }));
    });
    build_image(&root, &root.join("source"), |_| {}).unwrap();
    panic!("launcher crash fixture missed its callback");
}

#[test]
fn live_builder_is_retained_while_copying_and_partial_failure_cleans_up() {
    let _fork = os::fork_guard().unwrap();
    let root = crate::launch::tests::private_root().unwrap();
    let source = root.path().join("source");
    fs::write(&source, b"binary").unwrap();
    let (ready, started) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let root = root.path();
        let builder = scope.spawn(move || {
            HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move |path, output| {
                    output.write_all(b"partial")?;
                    ready.send(path.parent().unwrap().to_owned()).unwrap();
                    released.recv_timeout(Duration::from_secs(10)).unwrap();
                    Err(io::Error::from_raw_os_error(libc::EIO))
                }));
            });
            build_image(root, &source, |_| {})
        });
        let dir = started.recv_timeout(Duration::from_secs(10)).unwrap();
        let cache = state::Lock::index(&root.join("cache.lock")).unwrap();
        sweep_images(root, None);
        let preserved = dir.is_dir() && !dir.join("hive-launch").exists();
        drop(cache);
        release.send(()).unwrap();
        assert!(builder.join().unwrap().is_err());
        assert!(preserved);
        assert!(!dir.exists());
    });
}

#[test]
fn sweep_retains_unknown_unsafe_or_malformed_reservations() {
    for name in [
        ".hive-tmp-",
        ".hive-tmp-123",
        ".hive-tmp-0123456789ABCDEf",
        ".hive-tmp-0123456789abcdef",
        "unrelated",
    ] {
        let root = crate::launch::tests::private_root().unwrap();
        let dir = root.path().join("hive-launch-interrupted");
        state::directory(&dir).unwrap();
        let owner = state::Lock::acquire(&dir.join("owner.lock")).unwrap();
        let file = dir.join(name);
        state::private_file(&file, true).unwrap();
        if name == ".hive-tmp-0123456789abcdef" {
            // Even the exact helper spelling must satisfy private-file checks.
            fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        }
        drop(owner);
        let _cache = state::Lock::index(&root.path().join("cache.lock")).unwrap();
        sweep_images(root.path(), None);
        assert!(file.exists());
    }
}
