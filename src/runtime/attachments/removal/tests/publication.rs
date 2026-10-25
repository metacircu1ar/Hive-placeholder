use super::*;

fn publish(root: &Path, key: &str, name: &str) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(root).unwrap();
    file.write_all(b"snapshot").unwrap();
    file.as_file().sync_all().unwrap();
    crate::runtime::attachments::files::publish(root, key, name, file.into_temp_path())
}

#[test]
fn publication_and_dedup_refuse_extra_entries_and_preserve_the_existing_snapshot() {
    for existing in [false, true] {
        let temp = crate::runtime::test_directory();
        let root = temp.path().join("attachments");
        let (key, target) = fixture(&root);
        if !existing {
            fs::remove_file(&target).unwrap();
        }
        state::private_file(&root.join(&key).join("unexpected"), true).unwrap();
        assert!(publish(&root, &key, "file").is_err());
        assert_eq!(target.exists(), existing);
        assert!(root.join(&key).join("unexpected").exists());
        if existing {
            assert_eq!(fs::read(&target).unwrap(), b"snapshot");
        }
    }
}

#[test]
fn publication_rechecks_the_preflight_at_its_commit_boundary() {
    let temp = crate::runtime::test_directory();
    let root = temp.path().join("attachments");
    let (key, target) = fixture(&root);
    let expected = target.clone();
    let saved = root.join("saved");
    let retained = saved.clone();
    let result = state::sync_tests::with_hook(
        move |path, step| {
            if path == expected && step == Step::BeforePublish {
                fs::rename(&expected, &retained)?;
                state::private_file(&expected, true)
                    .unwrap()
                    .write_all(b"sentinel")?;
            }
            Ok(())
        },
        || publish(&root, &key, "file"),
    );
    assert!(result.is_err());
    assert_eq!(fs::read(target).unwrap(), b"sentinel");
    assert_eq!(fs::read(saved).unwrap(), b"snapshot");
}

#[test]
fn empty_directory_reuse_and_unreserved_filenames_remain_supported() {
    let temp = crate::runtime::test_directory();
    let root = temp.path().join("attachments");
    state::directory(&root).unwrap();
    let key = "d".repeat(64);
    for name in [
        "ordinary",
        ".hive-delete",
        ".hive-delete-2",
        ".hive-tmp-0123456789abcdef",
    ] {
        state::directory(&root.join(&key)).unwrap();
        publish(&root, &key, name).unwrap();
        publish(&root, &key, name).unwrap();
        assert_eq!(fs::read(root.join(&key).join(name)).unwrap(), b"snapshot");
        remove_snapshot(&root, &key, name).unwrap();
    }
}
