use super::*;

#[test]
fn accepted_replacement_retries_sync_without_changing_inode_or_mode() {
    let (_temp, root) = fixture();
    group(&root, b"old", Some(b"doc")).apply().unwrap();
    // Recovery must accept the same user-created directories as publication.
    if !rustix::process::geteuid().is_root() {
        for directory in [&root, &root.join("bundle"), &root.join("bundle/nested")] {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o775)).unwrap();
        }
    }
    let target = root.join("SKILL.md");
    let hooks = fail(target.clone(), Step::AfterPublish);
    let error = group(&root, b"new", Some(b"doc")).apply().unwrap_err();
    assert!(native(&error).published());
    assert_eq!(native(&error).io_error().raw_os_error(), Some(libc::EIO));
    drop(hooks);
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
    let before = fs::metadata(&target).unwrap();
    for step in [Step::BeforeFileSync, Step::BeforeDirectorySync] {
        let hooks = fail(target.clone(), step);
        let error = group(&root, b"new", Some(b"doc")).apply().unwrap_err();
        assert!(native(&error).published());
        assert_eq!(native(&error).io_error().raw_os_error(), Some(libc::EIO));
        drop(hooks);
    }
    assert!(group(&root, b"new", Some(b"doc")).apply().unwrap());
    assert!(!group(&root, b"new", Some(b"doc")).apply().unwrap());
    let after = fs::metadata(target).unwrap();
    assert_eq!((after.ino(), after.mode()), (before.ino(), before.mode()));
}

#[test]
fn removed_path_stays_in_old_intent_until_nested_parent_is_durable() {
    let (_temp, root) = fixture();
    group(&root, b"guide", Some(b"doc")).apply().unwrap();
    let path = root.join("bundle/nested/doc.md");
    let record = root.join(".hive-owned.json");
    let hooks = fail(path.clone(), Step::AfterRemove);
    let error = group(&root, b"guide", None).apply().unwrap_err();
    assert!(native(&error).published());
    assert!(!path.exists());
    drop(hooks);
    let intent = fs::read(&record).unwrap();
    assert!(String::from_utf8_lossy(&intent).contains("bundle/nested/doc.md"));
    // Two failed retries must not erase the absent path from recovery evidence.
    for _ in 0..2 {
        let hooks = fail(path.clone(), Step::BeforeDirectorySync);
        let error = group(&root, b"guide", None).apply().unwrap_err();
        assert_eq!(native(&error).io_error().raw_os_error(), Some(libc::EIO));
        assert_eq!(fs::read(&record).unwrap(), intent);
        drop(hooks);
    }
    // Retarget and interrupt after publishing the replacement intent; it is safe
    // for that intent to omit the absent path because its barrier has completed.
    let hooks = fail(
        record.clone(),
        hive_setup_support::checkpoints::Checkpoint::AfterIntent,
    );
    assert!(group(&root, b"retargeted", None).apply().is_err());
    drop(hooks);
    assert!(!String::from_utf8_lossy(&fs::read(&record).unwrap()).contains("bundle/nested/doc.md"));
    group(&root, b"retargeted", None).apply().unwrap();
    assert!(!group(&root, b"retargeted", None).apply().unwrap());
}

#[test]
fn missing_nested_directory_recovery_never_recreates_removed_directories() {
    let (_temp, root) = fixture();
    group(&root, b"guide", Some(b"doc")).apply().unwrap();
    let path = root.join("bundle/nested/doc.md");
    let hooks = fail(path.clone(), Step::AfterRemove);
    assert!(group(&root, b"guide", None).apply().is_err());
    drop(hooks);
    fs::remove_dir(root.join("bundle/nested")).unwrap();
    fs::remove_dir(root.join("bundle")).unwrap();
    let hooks = fail(path, Step::BeforeDirectorySync);
    assert!(group(&root, b"guide", None).apply().is_err());
    drop(hooks);
    group(&root, b"guide", None).apply().unwrap();
    assert!(!root.join("bundle").exists());
}

#[test]
fn current_group_repeats_data_and_parent_sync_without_rewriting() {
    let (_temp, root) = fixture();
    group(&root, b"guide", Some(b"doc")).apply().unwrap();
    let path = root.join("bundle/nested/doc.md");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let before = fs::metadata(&path).unwrap();
    for step in [Step::BeforeFileSync, Step::BeforeDirectorySync] {
        let hooks = fail(path.clone(), step);
        assert!(group(&root, b"guide", Some(b"doc")).apply().is_err());
        drop(hooks);
    }
    assert!(!group(&root, b"guide", Some(b"doc")).apply().unwrap());
    let after = fs::metadata(path).unwrap();
    assert_eq!((after.ino(), after.mode()), (before.ino(), before.mode()));
}
