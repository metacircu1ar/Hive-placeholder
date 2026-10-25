use super::*;

#[test]
fn late_editor_and_no_replace_creator_are_preserved() {
    for (existing, replacement) in [
        (true, b"edited".as_slice()),
        (false, b"late creator".as_slice()),
    ] {
        let (_temp, root) = fixture();
        if existing {
            group(&root, b"old", None).apply().unwrap();
        }
        let candidate = group(&root, b"new", None);
        let target = root.join("SKILL.md");
        let checked = target.clone();
        let hooks = Hooks::new(move |path, step| {
            if path == checked && step == Step::BeforePublish {
                fs::write(path, replacement).unwrap();
            }
            Ok(())
        });
        let error = candidate.apply().unwrap_err();
        assert_eq!(error.code(), "config_invalid");
        assert!(!native(&error).published());
        drop(hooks);
        assert_eq!(fs::read(target).unwrap(), replacement);
    }
}

#[test]
fn before_publish_failure_preserves_old_content_and_native_cause() {
    let (_temp, root) = fixture();
    group(&root, b"old", None).apply().unwrap();
    let target = root.join("SKILL.md");
    let hooks = fail(target.clone(), Step::BeforePublish);
    let error = group(&root, b"new", None).apply().unwrap_err();
    assert!(!native(&error).published());
    assert_eq!(native(&error).io_error().raw_os_error(), Some(libc::EIO));
    drop(hooks);
    assert_eq!(fs::read(target).unwrap(), b"old");
}

#[test]
fn inode_and_symlink_changes_during_recovery_preserve_foreign_entries() {
    for symlink in [false, true] {
        let (_temp, root) = fixture();
        group(&root, b"guide", None).apply().unwrap();
        let candidate = group(&root, b"guide", None);
        let target = root.join("SKILL.md");
        let original = root.join("original");
        let saved = original.clone();
        let checked = target.clone();
        let hooks = Hooks::new(move |path, step| {
            if path == checked && step == Step::BeforeFileSync {
                fs::rename(path, &saved).unwrap();
                if symlink {
                    std::os::unix::fs::symlink(&saved, path).unwrap();
                } else {
                    fs::write(path, b"guide").unwrap();
                }
            }
            Ok(())
        });
        assert_eq!(candidate.apply().unwrap_err().code(), "config_invalid");
        drop(hooks);
        assert_eq!(fs::read(&original).unwrap(), b"guide");
        assert_eq!(
            fs::symlink_metadata(target)
                .unwrap()
                .file_type()
                .is_symlink(),
            symlink
        );
    }
}

#[test]
fn removal_rechecks_editor_content_before_unlink() {
    let (_temp, root) = fixture();
    group(&root, b"guide", Some(b"old")).apply().unwrap();
    let path = root.join("bundle/nested/doc.md");
    let target = path.clone();
    let hooks = Hooks::new(move |candidate, step| {
        if candidate == target && step == Step::BeforeRemove {
            fs::write(candidate, b"user edit").unwrap();
        }
        Ok(())
    });
    assert_eq!(
        group(&root, b"guide", None).apply().unwrap_err().code(),
        "config_invalid"
    );
    drop(hooks);
    assert_eq!(fs::read(path).unwrap(), b"user edit");
}

#[test]
fn ownership_writer_error_retains_accepted_native_and_cleanup_evidence() {
    use std::error::Error as _;
    let cause = atomic_fs::Error::new(errno(), true)
        .with_cleanup_error(errno())
        .with_retained(PathBuf::from("retained-owned-preparation"));
    // setup-support's public writer API embeds the complete helper in io::Error.
    let error = io(stdio::Error::other(cause));
    assert!(error.message().contains("accepted"));
    let wrapper = error
        .source()
        .unwrap()
        .downcast_ref::<stdio::Error>()
        .unwrap();
    let evidence = wrapper
        .get_ref()
        .unwrap()
        .downcast_ref::<atomic_fs::Error>()
        .unwrap();
    assert!(evidence.published());
    assert_eq!(evidence.io_error().raw_os_error(), Some(libc::EIO));
    assert_eq!(
        evidence.cleanup_error().as_ref().unwrap().raw_os_error(),
        Some(libc::EIO)
    );
    assert_eq!(
        evidence.retained(),
        Some(Path::new("retained-owned-preparation"))
    );
}

#[test]
fn removal_preserves_same_bytes_replacement_file_and_moved_parent() {
    for replace_parent in [false, true] {
        let (_temp, root) = fixture();
        group(&root, b"guide", Some(b"doc")).apply().unwrap();
        let candidate = group(&root, b"guide", None);
        let target = root.join("bundle/nested/doc.md");
        let saved = root.join("saved");
        let preserved = saved.clone();
        let watched = target.clone();
        let hooks = Hooks::new(move |path, step| {
            if path == watched && step == Step::BeforeRemove {
                if replace_parent {
                    fs::rename(path.parent().unwrap(), &preserved)?;
                    fs::create_dir(path.parent().unwrap())?;
                } else {
                    fs::rename(path, &preserved)?;
                }
                fs::write(path, b"doc")?;
            }
            Ok(())
        });
        assert!(candidate.apply().is_err());
        drop(hooks);
        assert_eq!(fs::read(target).unwrap(), b"doc");
        assert_eq!(
            fs::read(if replace_parent {
                saved.join("doc.md")
            } else {
                saved
            })
            .unwrap(),
            b"doc"
        );
        let record: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(".hive-owned.json")).unwrap()).unwrap();
        assert!(record["pending"].is_object());
    }
}

#[test]
fn removal_cannot_finalize_after_its_parent_is_replaced_before_sync() {
    let (_temp, root) = fixture();
    group(&root, b"guide", Some(b"doc")).apply().unwrap();
    let target = root.join("bundle/nested/doc.md");
    let watched = target.clone();
    let moved = root.join("moved-parent");
    let saved = moved.clone();
    let hooks = Hooks::new(move |path, step| {
        if path == watched && step == Step::AfterRemove {
            assert!(!path.exists());
            fs::rename(path.parent().unwrap(), &saved)?;
            fs::create_dir(path.parent().unwrap())?;
        }
        Ok(())
    });
    let error = group(&root, b"guide", None).apply().unwrap_err();
    assert!(native(&error).published());
    drop(hooks);
    assert!(!target.exists());
    assert!(!moved.join("doc.md").exists());
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(".hive-owned.json")).unwrap()).unwrap();
    assert!(record["pending"].is_object());
    assert!(record["files"]["bundle/nested/doc.md"].is_string());
}
