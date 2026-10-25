use super::*;
use std::{error::Error as _, os::unix::fs::PermissionsExt, path::PathBuf};
mod process;

fn evidence(error: &crate::Error) -> &atomic_fs::Error {
    error.source().unwrap().downcast_ref().unwrap()
}

#[test]
fn json_publication_preserves_hook_context_and_commit_boundary() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("record.json");
    for phase in [Publish::Prepared, Publish::Renamed] {
        let error = write_json_hook(&path, &42, &mut |at| {
            if at == phase {
                let mut error = invalid("original hook context");
                error.request_id = Some("original-request".into());
                Err(error.caused_by(io::Error::from_raw_os_error(libc::EIO)))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(error.code, invalid("").code);
        assert_eq!(error.message, "original hook context");
        assert_eq!(error.request_id.as_deref(), Some("original-request"));
        assert_eq!(error.file_committed, phase == Publish::Renamed);
        assert_eq!(path.exists(), phase == Publish::Renamed);
        let original = evidence(&error)
            .io_error()
            .get_ref()
            .unwrap()
            .downcast_ref::<crate::Error>()
            .unwrap();
        assert_eq!(
            original
                .source()
                .unwrap()
                .downcast_ref::<io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(libc::EIO)
        );
    }
    assert_eq!(super::super::json::<u64>(&path).unwrap(), 42);
}

#[test]
fn record_bounds_precede_filesystem_effects_and_exceed_setup_support_bound() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("missing/record.json");
    assert!(write_json(&path, &"a".repeat(usize::try_from(RECORD_BYTES).unwrap())).is_err());
    assert!(!temp.path().join("missing").exists());
    let value = "a".repeat(usize::try_from(hive_setup_support::files::MAX_RECORD).unwrap() + 1);
    write_json(&path, &value).unwrap();
    assert_eq!(super::super::json::<String>(&path).unwrap(), value);
}

#[test]
fn prepared_hook_retains_prior_commit_while_outer_disk_boundary_has_its_own_policy() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("record.json");
    let error = write_json_hook(&path, &42, &mut |phase| {
        assert_eq!(phase, Publish::Prepared);
        Err(invalid("already accepted upstream").committed())
    })
    .unwrap_err();
    assert!(error.file_committed);
    assert!(!evidence(&error).published());
    assert!(!path.exists());
}

#[test]
fn native_faults_and_foreign_preparation_cleanup_are_retained() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("record.json");
    let guard = hooks::install(|_, step| {
        if step == Step::BeforeDirectorySync {
            Err(io::Error::from_raw_os_error(libc::ENOSPC))
        } else {
            Ok(())
        }
    });
    let error = write_json(&path, &42).unwrap_err();
    assert!(error.file_committed);
    assert_eq!(
        evidence(&error).io_error().raw_os_error(),
        Some(libc::ENOSPC)
    );
    drop(guard);
    let guard = hooks::install(|path, step| {
        if step == Step::BeforePublish {
            let temporary = fs::read_dir(path.parent().unwrap())?
                .map(|e| e.unwrap().path())
                .find(|p| {
                    p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(".comb-tmp-")
                })
                .unwrap();
            fs::rename(&temporary, temporary.with_extension("held"))?;
            fs::write(temporary, b"foreign")?;
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        Ok(())
    });
    let error = write_json(&path, &43).unwrap_err();
    assert!(!error.file_committed);
    assert!(evidence(&error).cleanup_error().is_some());
    assert_eq!(
        fs::read(evidence(&error).retained().as_ref().unwrap()).unwrap(),
        b"foreign"
    );
    assert_eq!(super::super::json::<u64>(&path).unwrap(), 42);
    drop(guard);
}

#[test]
fn directory_adapter_requires_modified_parent_and_rechecks_it_on_retry() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("search-only");
    directory(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o311)).unwrap();
    let first = directory(&path.join("new/store"));
    let retry = directory(&path.join("new/store"));
    let created = path.join("new").exists();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    if rustix::process::geteuid().as_raw() != 0 {
        assert!(first.is_err());
        assert!(retry.is_err());
        assert!(!created);
    }
}

#[test]
#[ignore = "subprocess fixture invoked by private_modes_with_permissive_umask"]
fn permissive_umask_child() {
    // SAFETY: this isolated child runs only this test; the parent never changes umask.
    unsafe {
        libc::umask(0);
    }
    let root = PathBuf::from(std::env::var_os("COMB_RECORD_MODE_ROOT").unwrap());
    let knowledge = root.join("knowledge");
    drop(crate::Comb::init(&knowledge).unwrap());
    crate::provision::select_none(&knowledge).unwrap();
    directory(Path::new("relative/records")).unwrap();
    assert!(root.join("relative/records").is_dir());
    let path = root.join("new/records/record.json");
    let _guard = hooks::install(|path, step| {
        if step == Step::Written {
            let stage = fs::read_dir(path.parent().unwrap())?
                .map(|e| e.unwrap().path())
                .find(|p| {
                    p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(".comb-tmp-")
                })
                .unwrap();
            assert_eq!(fs::metadata(stage)?.mode() & 0o777, 0o600);
        }
        Ok(())
    });
    write_json(&path, &42).unwrap();
    for directory in [root.join("new"), root.join("new/records")] {
        assert_eq!(fs::metadata(directory).unwrap().mode() & 0o777, 0o700);
    }
    for mode in [0o755, 0o750] {
        fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(mode)).unwrap();
        write_json(&path, &43).unwrap();
        assert_eq!(
            fs::metadata(path.parent().unwrap()).unwrap().mode() & 0o777,
            mode
        );
    }
}

#[test]
fn private_modes_with_permissive_umask() {
    let temp = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "provision::files::publication::tests::permissive_umask_child",
            "--ignored",
        ])
        .env("COMB_RECORD_MODE_ROOT", temp.path())
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn late_creator_wins_create_record() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("record.json");
    let _guard = hooks::install(|path, step| {
        if step == Step::BeforePublish {
            fs::write(path, b"37")?;
        }
        Ok(())
    });
    let error = create_record(&path, b"42").unwrap_err();
    assert!(!error.file_committed);
    assert_eq!(
        evidence(&error).io_error().kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(super::super::json::<u64>(&path).unwrap(), 37);
}

#[test]
fn group_writable_directory_allows_publication_and_resync_but_not_writable_records() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o775)).unwrap();
    let path = temp.path().join("record.json");
    if rustix::process::geteuid().is_root() {
        assert!(write_json(&path, &42).is_err());
        return;
    }
    write_json(&path, &42).unwrap();
    sync_json(&path, &42).unwrap();
    write_json(&path, &43).unwrap();
    assert_eq!(super::super::json::<u64>(&path).unwrap(), 43);
    assert_eq!(fs::metadata(temp.path()).unwrap().mode() & 0o777, 0o775);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o664)).unwrap();
    assert!(super::super::json::<u64>(&path).is_err());
}
