use super::*;

#[test]
fn replacements_and_new_siblings_at_unlink_boundary_are_preserved() {
    for defect in ["file", "directory", "root", "sibling"] {
        let temp = crate::runtime::test_directory();
        let root = temp.path().join("attachments");
        let (key, target) = fixture(&root);
        let bound = target.clone();
        let saved = temp.path().join("saved");
        let preserve = saved.clone();
        let location = root.clone();
        let child_key = key.clone();
        let result = state::sync_tests::with_hook(
            move |path, step| {
                if path == bound && step == Step::BeforeRemove {
                    match defect {
                        "file" => {
                            fs::rename(&bound, &preserve)?;
                            state::private_file(&bound, true)
                                .unwrap()
                                .write_all(b"replacement")?;
                        }
                        "directory" => {
                            fs::rename(location.join(&child_key), &preserve)?;
                            state::directory(&location.join(&child_key)).unwrap();
                            state::private_file(&bound, true)
                                .unwrap()
                                .write_all(b"replacement")?;
                        }
                        "root" => {
                            fs::rename(&location, &preserve)?;
                            state::directory(&location).unwrap();
                            state::directory(&location.join(&child_key)).unwrap();
                            state::private_file(&bound, true)
                                .unwrap()
                                .write_all(b"replacement")?;
                        }
                        "sibling" => {
                            state::private_file(&location.join(&child_key).join("extra"), true)
                                .unwrap();
                        }
                        _ => unreachable!(),
                    }
                }
                Ok(())
            },
            || remove_snapshot(&root, &key, "file"),
        );
        assert!(result.is_err(), "{defect}");
        assert_eq!(
            fs::read(&target).unwrap(),
            if defect == "sibling" {
                b"snapshot".as_slice()
            } else {
                b"replacement"
            }
        );
        if defect != "sibling" {
            let original = match defect {
                "file" => saved,
                "directory" => saved.join("file"),
                _ => saved.join(&key).join("file"),
            };
            assert_eq!(fs::read(original).unwrap(), b"snapshot");
        }
    }
}

#[test]
fn an_entry_appearing_after_absent_key_sync_is_not_acknowledged() {
    let temp = crate::runtime::test_directory();
    let root = temp.path().join("attachments");
    state::directory(&root).unwrap();
    let key = "c".repeat(64);
    let location = root.clone();
    let added = root.join(&key);
    let result = state::sync_tests::with_hook(
        move |path, step| {
            if path == location && step == Step::AfterDirectorySync {
                state::directory(&added).unwrap();
            }
            Ok(())
        },
        || remove_snapshot(&root, &key, "file"),
    );
    assert!(result.is_err());
    assert!(root.join(key).is_dir());
}
