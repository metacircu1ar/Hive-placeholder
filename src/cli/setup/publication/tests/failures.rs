use super::*;

#[test]
fn publication_failure_reports_the_affected_output_and_preserves_old_bytes() {
    for backup in [true, false] {
        for boundary in [Step::Written, Step::BeforeDirectorySync] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("config.yaml");
            fs::write(&path, OLD).unwrap();
            let snapshot = Snapshot::inspect(&path).unwrap();
            let target = snapshot.target.clone();
            let error = with(
                move |at, step| {
                    if (at != target) == backup && step == boundary {
                        return Err(native::Error::from_raw_os_error(libc::EIO));
                    }
                    Ok(())
                },
                || snapshot.publish(NEW),
            )
            .unwrap_err();
            let accepted = boundary == Step::BeforeDirectorySync;
            let details = evidence(&error).unwrap();
            assert_eq!(details.published(), accepted);
            assert_eq!(details.io_error().raw_os_error(), Some(libc::EIO));
            assert!(details.cleanup_error().is_none());
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                if !backup && accepted { NEW } else { OLD }
            );
            assert_eq!(
                backups(temp.path()).unwrap().len(),
                usize::from(!backup || accepted)
            );
            if backup {
                assert!(error.message().contains("backup"));
                assert!(error.message().contains("configuration was not changed"));
            } else if accepted {
                assert!(error.message().contains("Configuration is published"));
            }
        }
    }
}

#[test]
fn concurrent_text_and_symlink_edits_are_preserved_at_publication() {
    for symlink in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("actual.yaml");
        let original = temp.path().join("config.yaml");
        let other = temp.path().join("other.yaml");
        fs::write(&target, OLD).unwrap();
        fs::write(&other, "auth_profiles: {}\n# other\n").unwrap();
        std::os::unix::fs::symlink(&target, &original).unwrap();
        let snapshot = Snapshot::inspect(&original).unwrap();
        let selected = snapshot.target.clone();
        let link = original.clone();
        let error = with(
            move |at, step| {
                if at == selected && step == Step::BeforePublish {
                    if symlink {
                        fs::remove_file(&link)?;
                        std::os::unix::fs::symlink(&other, &link)?;
                    } else {
                        fs::write(at, "auth_profiles: {}\n# editor\n")?;
                    }
                }
                Ok(())
            },
            || snapshot.publish(NEW),
        )
        .unwrap_err();
        assert_eq!(error.code(), "config_invalid");
        assert!(!evidence(&error).unwrap().published());
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            if symlink {
                OLD
            } else {
                "auth_profiles: {}\n# editor\n"
            }
        );
        assert_eq!(
            fs::read_to_string(&original).unwrap(),
            if symlink {
                "auth_profiles: {}\n# other\n"
            } else {
                "auth_profiles: {}\n# editor\n"
            }
        );
        assert_eq!(backups(temp.path()).unwrap().len(), 1);
    }
}

#[test]
fn new_configuration_and_backup_collisions_never_overwrite_foreign_bytes() {
    for backup in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        if backup {
            fs::write(&path, OLD).unwrap();
        }
        let snapshot = Snapshot::inspect(&path).unwrap();
        let target = snapshot.target.clone();
        let occupied = Rc::new(RefCell::new(None));
        let selected = occupied.clone();
        let error = with(
            move |at, step| {
                if (at != target) == backup && step == Step::Written {
                    fs::write(at, "auth_profiles: {}\n# foreign\n")?;
                    selected.borrow_mut().replace(at.to_owned());
                }
                Ok(())
            },
            || snapshot.publish(NEW),
        )
        .unwrap_err();
        assert!(!evidence(&error).unwrap().published());
        let occupied = occupied.borrow().clone().unwrap();
        assert_eq!(
            fs::read_to_string(&occupied).unwrap(),
            "auth_profiles: {}\n# foreign\n"
        );
        if backup {
            assert_eq!(
                evidence(&error).unwrap().io_error().kind(),
                native::ErrorKind::AlreadyExists
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), OLD);
        } else {
            assert_eq!(error.code(), "config_invalid");
        }
    }
}
