use super::*;
use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink},
    os::unix::process::ExitStatusExt,
    time::Duration,
};

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, File) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().join("raw.log");
    let file = OpenOptions::new()
        .read(true)
        .append(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    (dir, path, file)
}

#[test]
fn raw_worker_keeps_its_lifetime_owner_through_actual_exit() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct Lifetime(Arc<AtomicBool>);
    impl Drop for Lifetime {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let (_dir, path, writer) = fixture();
    let released = Arc::new(AtomicBool::new(false));
    let owner = Owner::admit(&path, &writer, Lifetime(released.clone()))
        .unwrap()
        .unwrap();
    assert!(!released.load(Ordering::SeqCst));
    let guard = owner.start().unwrap();
    assert!(!released.load(Ordering::SeqCst));
    assert!(guard.finish(Duration::from_secs(1)));
    assert!(released.load(Ordering::SeqCst));
}
fn flood(file: &mut File) {
    file.write_all(&vec![b'x'; usize::try_from(TRIM_BYTES).unwrap() + 1])
        .unwrap();
    file.write_all(b"native crash evidence\n").unwrap();
}
#[test]
fn retained_tail_and_live_append_descriptor_survive_repeated_trims() {
    let (_dir, path, mut writer) = fixture();
    let original = writer.metadata().unwrap();
    flood(&mut writer);
    let mut owner = Owner::admit(&path, &writer, ()).unwrap().unwrap();
    assert_eq!(writer.metadata().unwrap().len(), 0);
    let tail = std::fs::read(path.with_extension("log.prev")).unwrap();
    assert_eq!(tail.len(), TAIL_BYTES);
    assert!(tail.ends_with(b"native crash evidence\n"));
    writer.write_all(b"still live\n").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"still live\n");
    assert!(paths::matches(
        &original,
        &rustix::fs::fstat(&writer).unwrap()
    ));
    flood(&mut writer);
    assert!(owner.trim().unwrap());
    writer.write_all(b"again\n").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"again\n");
}
#[test]
fn second_owner_never_trims_and_replacement_is_not_truncated() {
    let (_dir, path, mut writer) = fixture();
    let mut owner = Owner::admit(&path, &writer, ()).unwrap().unwrap();
    flood(&mut writer);
    assert!(Owner::admit(&path, &writer, ()).unwrap().is_none());
    let length = writer.metadata().unwrap().len();
    let old = path.with_extension("old");
    std::fs::rename(&path, &old).unwrap();
    let mut other = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    other.write_all(b"replacement").unwrap();
    assert!(owner.trim().is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
    assert_eq!(std::fs::metadata(old).unwrap().len(), length);
}
#[test]
fn unsafe_previous_and_failed_publication_preserve_active_output() {
    struct Fail;
    impl atomic_fs::Barriers for Fail {
        fn observe(&self, step: atomic_fs::Step) -> io::Result<()> {
            if step == atomic_fs::Step::BeforePublish {
                return Err(io::ErrorKind::WriteZero.into());
            }
            Ok(())
        }
    }
    let (_dir, path, mut writer) = fixture();
    let mut owner = Owner::admit(&path, &writer, ()).unwrap().unwrap();
    flood(&mut writer);
    let size = writer.metadata().unwrap().len();
    let previous = path.with_extension("log.prev");
    symlink(&path, &previous).unwrap();
    assert!(owner.trim().is_err());
    assert_eq!(writer.metadata().unwrap().len(), size);
    std::fs::remove_file(&previous).unwrap();
    assert!(owner.trim_with(&Fail).is_err());
    assert_eq!(writer.metadata().unwrap().len(), size);
    assert!(!previous.exists());
    // A failed truncate also retains the active bytes after the successful snapshot.
    owner.active = File::open(&path).unwrap();
    assert!(owner.trim().is_err());
    assert_eq!(writer.metadata().unwrap().len(), size);
    assert_eq!(
        std::fs::metadata(previous).unwrap().len(),
        TAIL_BYTES as u64
    );
}
#[test]
fn inherited_nonappend_foreground_and_unsafe_files_do_not_acquire_owner() {
    let (dir, path, writer) = fixture();
    let read_only = File::open(&path).unwrap();
    assert!(Owner::admit(&path, &read_only, ()).is_err());
    let unrelated = File::open("/dev/null").unwrap();
    assert!(Owner::admit(&path, &unrelated, ()).unwrap().is_none());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Owner::admit(&path, &writer, ()).is_err());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}
#[test]
fn timer_trims_a_finite_flood_and_releases_ownership_at_shutdown() {
    let (_dir, path, mut writer) = fixture();
    let guard = Owner::admit(&path, &writer, ())
        .unwrap()
        .unwrap()
        .start()
        .unwrap();
    flood(&mut writer);
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while writer.metadata().unwrap().len() > TRIM_BYTES {
        assert!(std::time::Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(guard.finish(Duration::from_secs(1)));
    // A child that another test is spawning holds a copy of the lock descriptor
    // until its exec closes it, so the released lock can stay held briefly.
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while Owner::admit(&path, &writer, ()).unwrap().is_none() {
        assert!(
            std::time::Instant::now() < until,
            "ownership was not released"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn raw_child_crash_fixture() {
    let Some(path) = std::env::var_os("HIVE_RAW_CRASH_FIXTURE") else {
        return;
    };
    let _owner = Owner::admit(Path::new(&path), &std::io::stderr(), ())
        .unwrap()
        .unwrap();
    std::io::stderr()
        .write_all(b"native crash evidence\n")
        .unwrap();
    rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::KILL)
        .unwrap();
    // Self-signal delivery can follow kill's return on macOS. Keep stderr
    // unchanged while awaiting termination instead of racing it with a panic.
    loop {
        std::thread::park();
    }
}

#[test]
fn native_crash_releases_lock_without_erasing_the_final_native_write() {
    let (_dir, path, writer) = fixture();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "raw::tests::raw_child_crash_fixture",
            "--nocapture",
        ])
        .env("HIVE_RAW_CRASH_FIXTURE", &path)
        .stdout(std::process::Stdio::null())
        .stderr(writer.try_clone().unwrap())
        .status()
        .unwrap();
    assert_eq!(
        status.signal(),
        Some(rustix::process::Signal::KILL.as_raw())
    );
    assert!(
        std::fs::read(&path)
            .unwrap()
            .ends_with(b"native crash evidence\n")
    );
    assert!(Owner::admit(&path, &writer, ()).unwrap().is_some());
}

#[test]
fn ongoing_append_can_overshoot_but_timer_keeps_trimming_the_same_inode() {
    let (_dir, path, mut writer) = fixture();
    let guard = Owner::admit(&path, &writer, ())
        .unwrap()
        .unwrap()
        .start()
        .unwrap();
    flood(&mut writer);
    // Soft retention permits this overshoot; do not assert an impossible hard cap.
    assert!(writer.metadata().unwrap().len() > TRIM_BYTES);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut previous = writer.metadata().unwrap().len();
    let buffer = vec![b'q'; 65536];
    loop {
        writer.write_all(&buffer).unwrap();
        std::thread::sleep(Duration::from_millis(10));
        let current = writer.metadata().unwrap().len();
        if current < previous {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        previous = current;
    }
    writer.write_all(b"append after trim\n").unwrap();
    assert!(
        std::fs::read(&path)
            .unwrap()
            .ends_with(b"append after trim\n")
    );
    assert!(guard.finish(Duration::from_secs(1)));
}

#[test]
fn replaced_directory_and_removed_owner_lock_stop_trimming() {
    let (_dir, path, mut writer) = fixture();
    let mut owner = Owner::admit(&path, &writer, ()).unwrap().unwrap();
    flood(&mut writer);
    let size = writer.metadata().unwrap().len();
    std::fs::remove_file(path.parent().unwrap().join(".raw.log.raw-owner.lock")).unwrap();
    assert!(owner.trim().is_err());
    assert_eq!(writer.metadata().unwrap().len(), size);
    drop(owner);
    let mut owner = Owner::admit(&path, &writer, ()).unwrap().unwrap();
    flood(&mut writer);
    let size = writer.metadata().unwrap().len();
    let moved = path.parent().unwrap().with_extension("moved");
    std::fs::rename(path.parent().unwrap(), &moved).unwrap();
    assert!(owner.trim().is_err());
    assert_eq!(writer.metadata().unwrap().len(), size);
    std::fs::rename(moved, path.parent().unwrap()).unwrap();
}
