use super::{read, stable};
use crate::{ErrorCode, service::auth::Connection};
use std::{
    cell::Cell,
    fs::{self, File, FileTimes},
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant, SystemTime},
};
type TestResult = Result<(), Box<dyn std::error::Error>>;

fn fixture(bytes: &[u8]) -> std::io::Result<(tempfile::TempDir, PathBuf)> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("private-page");
    fs::write(&path, bytes)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok((directory, path))
}
fn rewrite(path: &Path, bytes: &[u8], tick: u64) -> std::io::Result<()> {
    fs::write(path, bytes)?;
    File::open(path)?.set_times(
        FileTimes::new().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(tick)),
    )
}

#[test]
fn stable_reads_keep_size_bounds_missing_file_and_native_causes() -> TestResult {
    let (directory, path) = fixture(b"abc")?;
    assert_eq!(read(&path, 3)?, b"abc");
    let Err(oversized) = read(&path, 2) else {
        panic!("oversized read succeeded")
    };
    assert_eq!(oversized.code, ErrorCode::SizeLimit);
    let Err(missing) = read(&directory.path().join("absent"), 3) else {
        panic!("missing read succeeded")
    };
    assert!(missing.is_missing_file());
    let result = stable::with(
        |_| Err(std::io::Error::from_raw_os_error(libc::EIO)),
        || read(&path, 3),
    );
    let Err(error) = result else {
        panic!("injected native failure succeeded")
    };
    assert_eq!(
        crate::error::tests::io_cause(&error).raw_os_error(),
        Some(libc::EIO)
    );
    Ok(())
}

#[test]
fn changed_content_and_atomic_replacements_are_reopened() -> TestResult {
    for replace in [false, true] {
        let (_directory, path) = fixture(b"old")?;
        let count = Rc::new(Cell::new(0));
        let observed = count.clone();
        let bytes = stable::with(
            move |path| {
                observed.set(observed.get() + 1);
                if observed.get() == 1 {
                    if replace {
                        let staged = path.with_extension("prepared");
                        fs::write(&staged, b"new")?;
                        fs::rename(staged, path)?;
                    } else {
                        rewrite(path, b"new", 1)?;
                    }
                }
                Ok(())
            },
            || read(&path, 3),
        )?;
        assert_eq!(bytes, b"new");
        assert_eq!(count.get(), 2);
    }
    Ok(())
}

#[test]
fn changed_oversize_file_can_retry_but_disappearance_never_returns_stale_bytes() -> TestResult {
    let (_directory, path) = fixture(b"oversized")?;
    let count = Rc::new(Cell::new(0));
    let observed = count.clone();
    let bytes = stable::with(
        move |path| {
            observed.set(observed.get() + 1);
            if observed.get() == 1 {
                rewrite(path, b"new", 1)?;
            }
            Ok(())
        },
        || read(&path, 3),
    )?;
    assert_eq!(bytes, b"new");
    assert_eq!(count.get(), 2);
    let result = stable::with(|path| fs::remove_file(path), || read(&path, 3));
    let Err(error) = result else {
        panic!("disappearance returned stale bytes")
    };
    assert!(error.is_missing_file());
    Ok(())
}

#[test]
fn persistent_churn_is_bounded_and_credential_consumers_keep_wire_privacy() -> TestResult {
    for credential in [false, true] {
        let (_directory, path) = fixture(b"private credential secret")?;
        let count = Rc::new(Cell::new(0));
        let observed = count.clone();
        let result = stable::with(
            move |path| {
                observed.set(observed.get() + 1);
                rewrite(path, b"private credential secret", observed.get())
            },
            || {
                if credential {
                    Connection::read(&path).map(|_| ())
                } else {
                    read(&path, 64).map(|_| ())
                }
            },
        );
        let Err(error) = result else {
            panic!("churn was accepted")
        };
        assert_eq!(error.code, ErrorCode::Conflict);
        assert_eq!(count.get(), 3);
        let wire = serde_json::to_string(&error)?;
        assert!(!wire.contains("private credential secret"));
        assert!(!wire.contains("private-page"));
    }
    Ok(())
}

#[test]
fn unsafe_names_links_permissions_and_owners_are_refused() -> TestResult {
    for kind in 0..3 {
        let (_directory, path) = fixture(b"old")?;
        let result = stable::with(
            move |path| {
                match kind {
                    0 => {
                        let other = path.with_extension("other");
                        fs::write(&other, b"foreign")?;
                        fs::remove_file(path)?;
                        symlink(other, path)?;
                    }
                    1 => fs::hard_link(path, path.with_extension("link"))?,
                    _ => fs::set_permissions(path, fs::Permissions::from_mode(0o666))?,
                }
                Ok(())
            },
            || read(&path, 3),
        );
        let Err(error) = result else {
            panic!("unsafe changed file accepted")
        };
        assert_eq!(error.code, ErrorCode::UnsafePath);
    }
    if !rustix::process::geteuid().is_root() {
        let Err(error) = read(Path::new("/etc/passwd"), 1024) else {
            panic!("foreign owner accepted")
        };
        assert_eq!(error.code, ErrorCode::UnsafePath);
    }
    Ok(())
}

#[test]
fn changed_ctime_detects_a_same_sized_rewrite_with_restored_mtime() -> TestResult {
    let (_directory, path) = fixture(b"old")?;
    let before = fs::metadata(&path)?;
    let modified = before.modified()?;
    let changed_at = (before.ctime(), before.ctime_nsec());
    let count = Rc::new(Cell::new(0));
    let observed = count.clone();
    let bytes = stable::with(
        move |path| {
            observed.set(observed.get() + 1);
            if observed.get() == 1 {
                // A fast rewrite can share one filesystem timestamp tick.
                // This case specifically proves detection when ctime differs.
                let deadline = Instant::now() + Duration::from_secs(2);
                loop {
                    fs::write(path, b"new")?;
                    File::open(path)?.set_times(FileTimes::new().set_modified(modified))?;
                    let after = fs::metadata(path)?;
                    assert_eq!(after.modified()?, modified);
                    if (after.ctime(), after.ctime_nsec()) != changed_at {
                        break;
                    }
                    assert!(Instant::now() < deadline, "fixture ctime did not advance");
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            Ok(())
        },
        || read(&path, 3),
    )?;
    assert_eq!(bytes, b"new");
    assert_eq!(count.get(), 2);
    let after = fs::metadata(path)?;
    assert_eq!(after.modified()?, modified);
    assert_ne!((after.ctime(), after.ctime_nsec()), changed_at);
    assert_eq!(after.dev(), before.dev());
    assert_eq!(after.ino(), before.ino());
    assert_eq!(after.len(), before.len());
    Ok(())
}
