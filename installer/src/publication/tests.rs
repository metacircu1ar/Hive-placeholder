use super::*;
use std::{io::Read, os::unix::fs::PermissionsExt};

#[test]
fn complete_manager_survives_accepted_failures_and_retry_repeats_barriers() {
    for point in [
        "manager_before_publish",
        "manager_after_publish",
        "manager_before_sync",
        "manager_after_sync",
    ] {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path().join("prefix");
        let result = crate::publish::manager(&prefix, &mut |at| {
            if at == point {
                Err("manager fault".into())
            } else {
                Ok(())
            }
        });
        let error = result.unwrap_err();
        let native = error.downcast_ref::<atomic_fs::Error>().unwrap();
        assert_eq!(native.published(), point != "manager_before_publish");
        let manager = prefix.join("lib/hive");
        if native.published() {
            assert_eq!(
                fs::read(manager.join("owner")).unwrap(),
                crate::publish::OWNER
            );
            assert!(manager.join("generations").is_dir());
        } else {
            assert!(!manager.exists());
        }
        let mut events = vec![];
        crate::publish::manager(&prefix, &mut |event| {
            events.push(event.to_owned());
            Ok(())
        })
        .unwrap();
        assert!(events.contains(&format!("parent_synced:{}", prefix.join("lib").display())));
    }
}

#[test]
fn incomplete_or_foreign_manager_is_never_adopted() {
    for bytes in [None, Some(b"foreign".as_slice())] {
        let root = tempfile::tempdir().unwrap();
        let manager = root.path().join("lib/hive");
        fs::create_dir_all(&manager).unwrap();
        if let Some(bytes) = bytes {
            fs::write(manager.join("owner"), bytes).unwrap();
        }
        assert!(crate::publish::manager(root.path(), &mut |_| Ok(())).is_err());
        assert!(!manager.join("generations").exists());
    }
}

struct CheckedReader<'a> {
    destination: &'a Path,
    mode: u32,
    fail: bool,
    read: bool,
}
impl Read for CheckedReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        assert!(!self.destination.exists());
        let entries =
            fs::read_dir(self.destination.parent().unwrap())?.collect::<io::Result<Vec<_>>>()?;
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].metadata()?.permissions().mode() & 0o777,
            self.mode
        );
        if self.read {
            return if self.fail {
                Err(io::Error::other("source failed"))
            } else {
                Ok(0)
            };
        }
        self.read = true;
        buffer[..4].copy_from_slice(b"data");
        Ok(4)
    }
}

#[test]
fn payload_is_complete_and_mode_is_final_before_first_byte() {
    for mode in [0o644, 0o755] {
        for fail in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let destination = root.path().join("payload");
            let mut reader = CheckedReader {
                destination: &destination,
                mode,
                fail,
                read: false,
            };
            let result = stream(
                &destination,
                mode,
                &mut reader,
                4,
                &atomic_fs::DefaultBarriers,
            );
            assert_eq!(result.is_err(), fail);
            if fail {
                assert!(!destination.exists());
                assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
            } else {
                assert_eq!(fs::read(&destination).unwrap(), b"data");
                let error = stream(
                    &destination,
                    mode,
                    &mut b"other".as_slice(),
                    5,
                    &atomic_fs::DefaultBarriers,
                )
                .unwrap_err();
                assert!(!error.published());
                assert_eq!(fs::read(&destination).unwrap(), b"data");
            }
        }
    }
}

#[test]
fn incorrect_length_never_publishes_and_cleanup_keeps_primary_error() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("payload");
    let error = stream(
        &path,
        0o755,
        &mut b"short".as_slice(),
        10,
        &atomic_fs::DefaultBarriers,
    )
    .unwrap_err();
    assert!(!error.published());
    assert!(!path.exists());
    let error = failed(Box::new(error), "cleanup", Err("secondary".into()));
    assert!(
        error
            .source()
            .unwrap()
            .downcast_ref::<atomic_fs::Error>()
            .is_some()
    );
    assert!(error.to_string().contains("secondary"));
}

#[test]
fn pending_or_selected_generation_is_never_discarded() {
    let root = tempfile::tempdir().unwrap();
    let manager = crate::publish::manager(root.path(), &mut |_| Ok(())).unwrap();
    let generation = manager.join("generations/g-kept");
    fs::create_dir(&generation).unwrap();
    fs::write(manager.join("pending-install.json"), b"possibly accepted").unwrap();
    discard_unreferenced(&manager, &generation).unwrap();
    assert!(generation.exists());
    fs::remove_file(manager.join("pending-install.json")).unwrap();
    std::os::unix::fs::symlink("generations/g-kept", manager.join("current")).unwrap();
    discard_unreferenced(&manager, &generation).unwrap();
    assert!(generation.exists());
}

#[test]
fn file_modes_ignore_caller_umask() {
    for mask in [0, 0o077] {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "publication::tests::umask_fixture", "--ignored"])
            .env("HIVE_INSTALL_TEST_UMASK", mask.to_string())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
    }
}

#[test]
fn pointer_sync_failures_report_which_generation_is_visible() {
    struct Fault {
        calls: std::cell::Cell<usize>,
        at: usize,
    }
    impl Barriers for Fault {
        fn sync_directory(&self, file: &fs::File) -> io::Result<()> {
            let call = self.calls.replace(self.calls.get() + 1);
            if call == self.at {
                return Err(io::Error::other("pointer source sync"));
            }
            file.sync_all()
        }
    }
    for at in [0, 1] {
        let root = tempfile::tempdir().unwrap();
        let manager = crate::publish::manager(root.path(), &mut |_| Ok(())).unwrap();
        for name in ["g-before", "g-after"] {
            fs::create_dir(manager.join("generations").join(name)).unwrap();
        }
        crate::publish::replace(&manager, Path::new("generations/g-before")).unwrap();
        let error = crate::publish::replace_with(
            &manager,
            Path::new("generations/g-after"),
            &Fault {
                calls: std::cell::Cell::new(0),
                at,
            },
        )
        .unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<atomic_fs::Error>()
                .unwrap()
                .published(),
            at == 1
        );
        let expected = if at == 0 {
            "generations/g-before"
        } else {
            "generations/g-after"
        };
        assert_eq!(
            crate::publish::current(&manager).unwrap().as_deref(),
            Some(Path::new(expected))
        );
        assert!(manager.join("generations/g-before").is_dir());
        assert!(manager.join("generations/g-after").is_dir());
    }
}

#[test]
#[ignore = "Owned subprocess only: changing umask must not affect other tests"]
fn umask_fixture() {
    let mask = std::env::var("HIVE_INSTALL_TEST_UMASK")
        .unwrap()
        .parse()
        .unwrap();
    rustix::process::umask(rustix::fs::Mode::from_bits_retain(mask));
    payload_is_complete_and_mode_is_final_before_first_byte();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("metadata");
    write(&path, b"complete").unwrap();
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o644
    );
}
