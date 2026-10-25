use super::*;
use std::{cell::RefCell, os::unix::fs::PermissionsExt, path::PathBuf};
type Hook = Box<dyn FnMut(&Path, Step) -> io::Result<()>>;
thread_local! { static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) }; }
pub(super) fn checkpoint(path: &Path, step: Step) -> Result<()> {
    HOOK.with(|slot| match slot.borrow_mut().as_mut() {
        Some(hook) => hook(path, step),
        None => Ok(()),
    })
}
struct Guard;
fn hook(callback: impl FnMut(&Path, Step) -> Result<()> + 'static) -> Guard {
    HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(callback)));
    Guard
}
impl Drop for Guard {
    fn drop(&mut self) {
        HOOK.with(|slot| *slot.borrow_mut() = None);
    }
}
fn evidence(error: &io::Error) -> &atomic_fs::Error {
    error.get_ref().unwrap().downcast_ref().unwrap()
}

#[test]
fn publication_and_matching_retries_keep_native_failure_and_commit_evidence() {
    for replace in [false, true] {
        for phase in [
            Step::BeforeFileSync,
            Step::BeforePublish,
            Step::AfterPublish,
            Step::BeforeDirectorySync,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("record");
            let fault = hook(move |_, at| {
                if phase == at {
                    Err(io::Error::from_raw_os_error(libc::ENOSPC))
                } else {
                    Ok(())
                }
            });
            let error = write(&path, b"content", replace).unwrap_err();
            let published = matches!(phase, Step::AfterPublish | Step::BeforeDirectorySync);
            assert_eq!(error.kind(), io::ErrorKind::StorageFull);
            assert_eq!(
                evidence(&error).io_error().raw_os_error(),
                Some(libc::ENOSPC)
            );
            assert_eq!(evidence(&error).published(), published);
            assert_eq!(path.exists(), published);
            drop(fault);
            if published {
                for phase in [Step::BeforeFileSync, Step::BeforeDirectorySync] {
                    let fault = hook(move |_, at| {
                        if at == phase {
                            Err(io::Error::from_raw_os_error(libc::EIO))
                        } else {
                            Ok(())
                        }
                    });
                    assert!(evidence(&write(&path, b"content", replace).unwrap_err()).published());
                    drop(fault);
                }
                write(&path, b"content", replace).unwrap();
            }
        }
    }
}

#[test]
fn matching_retry_retains_inode_and_rejects_a_failed_required_directory_barrier() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("record");
    write(&path, b"content", false).unwrap();
    let inode = fs::metadata(&path).unwrap().ino();
    super::super::FAIL_DIRECTORY_SYNC
        .with(|fault| *fault.borrow_mut() = Some((temp.path().into(), libc::EROFS)));
    let result = write(&path, b"content", false);
    super::super::FAIL_DIRECTORY_SYNC.with(|fault| *fault.borrow_mut() = None);
    let error = result.unwrap_err();
    assert!(evidence(&error).published());
    assert_eq!(
        evidence(&error).io_error().raw_os_error(),
        Some(libc::EROFS)
    );
    write(&path, b"content", false).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
}

#[test]
fn late_creator_wins_and_cleanup_error_keeps_foreign_preparation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("record");
    let fault = hook(|path, at| {
        if at == Step::BeforePublish {
            fs::write(path, b"late creator")?;
        }
        Ok(())
    });
    let error = write(&path, b"ours", false).unwrap_err();
    assert!(!evidence(&error).published());
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(&path).unwrap(), b"late creator");
    drop(fault);
    let fault = hook(|path, at| {
        if at == Step::BeforePublish {
            let temporary = fs::read_dir(path.parent().unwrap())?
                .map(|e| e.unwrap().path())
                .find(|p| {
                    p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(".hive-tmp-")
                })
                .unwrap();
            fs::rename(&temporary, temporary.with_extension("held"))?;
            fs::write(temporary, b"foreign")?;
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        Ok(())
    });
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let error = write(&path, b"ours", true).unwrap_err();
    assert!(evidence(&error).cleanup_error().is_some());
    assert_eq!(
        fs::read(evidence(&error).retained().unwrap()).unwrap(),
        b"foreign"
    );
    drop(fault);
}

#[test]
fn concurrent_reader_acknowledges_whole_records_during_both_publication_modes() {
    for replace in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("record");
        let old = vec![b'a'; usize::try_from(MAX_RECORD).unwrap()];
        let new = vec![b'b'; usize::try_from(MAX_RECORD).unwrap()];
        if replace {
            write(&path, &old, false).unwrap();
        }
        let (request, requests) = std::sync::mpsc::channel();
        let (answer, answers) = std::sync::mpsc::channel();
        let reader_path = path.clone();
        let reader = std::thread::spawn(move || {
            while requests.recv().is_ok() {
                let bytes = super::super::read(&reader_path).unwrap();
                assert!(bytes.as_ref().is_none_or(|b| b == &old || b == &new));
                answer.send(bytes.map(|b| b[0])).unwrap();
            }
        });
        let guard = hook(move |_, step| {
            if matches!(step, Step::BeforePublish | Step::AfterPublish) {
                for _ in 0..10 {
                    request.send(()).unwrap();
                    let got = answers
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    assert_eq!(
                        got,
                        if step == Step::AfterPublish {
                            Some(b'b')
                        } else if replace {
                            Some(b'a')
                        } else {
                            None
                        }
                    );
                }
            }
            Ok(())
        });
        write(
            &path,
            &vec![b'b'; usize::try_from(MAX_RECORD).unwrap()],
            replace,
        )
        .unwrap();
        drop(guard);
        reader.join().unwrap();
    }
}

#[test]
#[ignore = "subprocess fixture invoked by record_death_boundaries"]
fn record_death_child() {
    let path = PathBuf::from(std::env::var_os("HIVE_RECORD_DEATH_PATH").unwrap());
    let phase = std::env::var("HIVE_RECORD_DEATH_PHASE").unwrap();
    let replace = std::env::var("HIVE_RECORD_DEATH_REPLACE").unwrap() == "true";
    let _guard = hook(move |_, step| {
        if format!("{step:?}") == phase {
            std::process::exit(71);
        }
        Ok(())
    });
    write(&path, b"new", replace).unwrap();
    panic!("death boundary was not reached");
}

#[test]
fn record_death_boundaries() {
    for replace in [false, true] {
        for phase in [
            Step::BeforeFileSync,
            Step::AfterFileSync,
            Step::BeforePublish,
            Step::AfterPublish,
            Step::BeforeDirectorySync,
            Step::AfterDirectorySync,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("record");
            if replace {
                write(&path, b"old", false).unwrap();
            }
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "files::publication::tests::record_death_child",
                    "--ignored",
                ])
                .env("HIVE_RECORD_DEATH_PATH", &path)
                .env("HIVE_RECORD_DEATH_PHASE", format!("{phase:?}"))
                .env("HIVE_RECORD_DEATH_REPLACE", replace.to_string())
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(71), "{output:?}");
            let published = matches!(
                phase,
                Step::AfterPublish | Step::BeforeDirectorySync | Step::AfterDirectorySync
            );
            let expected = if published {
                Some(b"new".to_vec())
            } else if replace {
                Some(b"old".to_vec())
            } else {
                None
            };
            assert_eq!(super::super::read(&path).unwrap(), expected);
            write(&path, b"new", replace).unwrap();
        }
    }
}
