use super::*;
use crate::disk::durable::hooks;
use atomic_fs::Step;
use std::{error::Error as _, os::unix::fs::MetadataExt};

#[test]
fn all_publication_faults_preserve_errno_commit_state_and_cleanup_evidence() {
    for (index, at) in [
        Step::Written,
        Step::BeforeFileSync,
        Step::AfterFileSync,
        Step::BeforePublish,
        Step::AfterPublish,
        Step::BeforeDirectorySync,
        Step::AfterDirectorySync,
    ]
    .into_iter()
    .enumerate()
    {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state");
        atomic(&path, b"before", &System).unwrap();
        let error = hooks::with(
            move |_, step| {
                if step == at {
                    Err(io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
            || atomic(&path, b"after", &System),
        )
        .unwrap_err();
        assert_io(&error, index >= 4, None);
        let failure = error
            .source()
            .unwrap()
            .downcast_ref::<atomic_fs::Error>()
            .unwrap();
        assert_eq!(failure.published(), index >= 4);
        assert!(failure.cleanup_error().is_none() && failure.retained().is_none());
        assert_eq!(
            fs::read(&path).unwrap(),
            if index >= 4 {
                b"after".as_slice()
            } else {
                b"before"
            }
        );
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[test]
fn private_preparation_and_final_flush_keep_native_errors_and_order() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().to_owned();
    let path = directory.join("state");
    let recording = Recording::default();
    recording.barrier(Ok(()));
    recording.fail(Operation::FullFlush, 1);
    let error = hooks::with(
        move |_, step| {
            if step == Step::Written {
                let entry = fs::read_dir(&directory).unwrap().next().unwrap().unwrap();
                assert!(
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".comb-tmp-")
                );
                assert_eq!(entry.metadata().unwrap().mode() & 0o777, 0o600);
            }
            Ok(())
        },
        || atomic(&path, b"after", &recording),
    )
    .unwrap_err();
    assert_io(&error, true, None);
    assert_eq!(fs::read(path).unwrap(), b"after");
    let mut expected = vec![Operation::Fsync];
    if cfg!(target_os = "macos") {
        expected.push(Operation::OrderingBarrier);
    }
    expected.extend([Operation::Fsync, Operation::FullFlush]);
    assert_eq!(recording.counts().events, expected);
}

#[test]
fn cleanup_failure_retains_preparation_and_original_native_error() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().to_owned();
    let path = directory.join("state");
    let error = hooks::with(
        move |_, step| {
            if step == Step::Written {
                let stage = fs::read_dir(&directory)
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                fs::remove_file(&stage)?;
                fs::create_dir(&stage)?;
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
            Ok(())
        },
        || atomic(&path, b"after", &System),
    )
    .unwrap_err();
    assert_io(&error, false, None);
    let failure = error
        .source()
        .unwrap()
        .downcast_ref::<atomic_fs::Error>()
        .unwrap();
    assert!(failure.cleanup_error().is_some());
    assert!(failure.retained().as_ref().unwrap().is_dir());
    assert!(!path.exists());
}

#[test]
fn new_page_publication_never_exposes_empty_target_or_clobbers_a_late_creator() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("page.md");
    let observed = path.clone();
    let checked = hooks::with(
        move |at, step| {
            if at == observed && step == Step::Written {
                assert!(!at.exists());
            }
            Ok(())
        },
        || {
            create_ordered_checked(&path, root.path(), b"our page", &System, &|| {
                assert!(!path.exists());
                // The editor wins after the caller's check but before native rename.
                fs::write(&path, b"editor's page")?;
                Ok(())
            })
        },
    )
    .unwrap_err();
    assert!(!checked.file_committed);
    assert_eq!(native_cause(&checked).kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(&path).unwrap(), b"editor's page");
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn administration_publication_rechecks_private_parent_at_acceptance() {
    use std::os::unix::fs::PermissionsExt;
    for mode in [0o750, 0o770] {
        let root = crate::test_support::private_directory().unwrap();
        let path = root.path().join("intent.json");
        let selected = path.clone();
        let error = crate::disk::publication_hooks::with(
            move |at, step| {
                if at == selected && step == Step::BeforePublish {
                    fs::set_permissions(at.parent().unwrap(), fs::Permissions::from_mode(mode))?;
                }
                Ok(())
            },
            || atomic_admin(&path, b"intent", &System).unwrap_err(),
        );
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(!error.file_committed);
        assert!(!path.exists());
    }
}

#[test]
fn administration_retry_refuses_shared_parent_without_relaxing_receipt_directory_policy() {
    use std::os::unix::fs::PermissionsExt;
    let root = crate::test_support::private_directory().unwrap();
    let path = root.path().join("record.json");
    atomic_admin(&path, b"complete", &System).unwrap();
    for mode in [0o750, 0o770] {
        fs::set_permissions(root.path(), fs::Permissions::from_mode(mode)).unwrap();
        assert!(sync_admin(&path, &System).is_err());
        if !rustix::process::geteuid().is_root() || mode == 0o750 {
            sync_existing(&path, &System, true).unwrap();
        }
        assert_eq!(fs::read(&path).unwrap(), b"complete");
    }
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    sync_admin(&path, &System).unwrap();
}
