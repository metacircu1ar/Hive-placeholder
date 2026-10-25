use super::*;

#[test]
fn accepted_configuration_retries_both_barriers_without_rewrite_or_backup() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.yaml");
    fs::write(&path, OLD).unwrap();
    let snapshot = Snapshot::inspect(&path).unwrap();
    let target = snapshot.target.clone();
    let error = with(
        move |at, step| {
            if at == target && step == Step::BeforeDirectorySync {
                return Err(native::Error::from_raw_os_error(libc::EIO));
            }
            Ok(())
        },
        || snapshot.publish(NEW),
    )
    .unwrap_err();
    assert!(evidence(&error).unwrap().published());
    let fresh = Snapshot::inspect(&path).unwrap();
    let before = fs::metadata(&path).unwrap();
    let backup = backups(temp.path()).unwrap();
    for boundary in [Step::BeforeFileSync, Step::BeforeDirectorySync] {
        let error = with(
            move |_, step| {
                if step == boundary {
                    return Err(native::Error::from_raw_os_error(libc::EIO));
                }
                Ok(())
            },
            || fresh.publish(NEW),
        )
        .unwrap_err();
        assert!(evidence(&error).unwrap().published());
        assert_eq!(
            evidence(&error).unwrap().io_error().raw_os_error(),
            Some(libc::EIO)
        );
    }
    let seen = Rc::new(RefCell::new(Vec::new()));
    let record = seen.clone();
    assert!(
        with(
            move |_, step| {
                record.borrow_mut().push(step);
                Ok(())
            },
            || fresh.publish(NEW)
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(
        *seen.borrow(),
        vec![
            Step::BeforeFileSync,
            Step::AfterFileSync,
            Step::BeforeDirectorySync,
            Step::AfterDirectorySync
        ]
    );
    assert_eq!(backups(temp.path()).unwrap(), backup);
    let after = fs::metadata(&path).unwrap();
    assert_eq!((before.ino(), before.mode()), (after.ino(), after.mode()));
    assert_eq!(fs::read_to_string(&path).unwrap(), NEW);
}

#[test]
fn identical_retry_refuses_identity_and_content_changes_during_barriers() {
    for replace in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        fs::write(&path, OLD).unwrap();
        let snapshot = Snapshot::inspect(&path).unwrap();
        let error = with(
            move |at, step| {
                if step == Step::BeforeFileSync {
                    if replace {
                        fs::remove_file(at)?;
                    }
                    fs::write(at, if replace { OLD } else { NEW })?;
                }
                Ok(())
            },
            || snapshot.publish(OLD),
        )
        .unwrap_err();
        assert_eq!(error.code(), "config_invalid");
        assert!(evidence(&error).unwrap().published());
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            if replace { OLD } else { NEW }
        );
        assert!(backups(temp.path()).unwrap().is_empty());
    }
}
