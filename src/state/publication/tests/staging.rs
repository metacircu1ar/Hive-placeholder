use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn current_shapes_are_recognized_without_legacy_aliases_or_deletion_authority() {
    let temp = crate::runtime::test_directory();
    for name in [".hive-tmp-ABC123", ".hive-tmp-0123456789abcdef"] {
        let path = temp.path().join(name);
        state::private_file(&path, true).unwrap();
        assert!(staging_file(&path).unwrap());
        assert!(path.exists());
    }
    for name in [
        ".tmpABC123",
        ".hive-tmp-",
        ".hive-tmp-ABCDE",
        ".hive-tmp-ABCDEFG",
        ".hive-tmp-0123456789abcdeF",
        ".hive-tmp-ABC_12",
        ".comb-tmp-ABC123",
    ] {
        let path = temp.path().join(name);
        state::private_file(&path, true).unwrap();
        assert!(!staging_file(&path).unwrap(), "{name}");
        assert!(path.exists());
    }
    let reservation = atomic_fs::PreparedCreate::new(
        temp.path().join("final"),
        hive_setup_support::names::PUBLICATION_OPTIONS,
    )
    .unwrap();
    let stages: Vec<_> = fs::read_dir(temp.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".hive-tmp-")
        })
        .filter(|path| staging_file(path).unwrap())
        .collect();
    assert_eq!(stages.len(), 3);
    reservation.cancel().unwrap();
}

#[test]
fn current_staging_names_still_reject_unsafe_types_permissions_and_links() {
    for defect in ["public", "symlink", "hardlink", "directory"] {
        let temp = crate::runtime::test_directory();
        let path = temp.path().join(".hive-tmp-ABC123");
        match defect {
            "directory" => state::directory(&path).unwrap(),
            "symlink" | "hardlink" => {
                let saved = temp.path().join("saved");
                state::private_file(&saved, true).unwrap();
                if defect == "symlink" {
                    std::os::unix::fs::symlink(&saved, &path).unwrap();
                } else {
                    fs::hard_link(saved, &path).unwrap();
                }
            }
            "public" => {
                state::private_file(&path, true).unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            }
            _ => unreachable!(),
        }
        assert_eq!(
            staging_file(&path).unwrap_err().code(),
            "unsafe_state",
            "{defect}"
        );
        assert!(path.symlink_metadata().is_ok());
    }
}

#[test]
fn unsafe_targets_and_ancestors_fail_without_publishing_bytes() {
    for defect in ["symlink", "hardlink", "public", "directory", "loop"] {
        let (_temp, path) = fixture();
        let saved = path.with_file_name("saved");
        fs::rename(&path, &saved).unwrap();
        let target = match defect {
            "symlink" => {
                std::os::unix::fs::symlink(&saved, &path).unwrap();
                path.clone()
            }
            "hardlink" => {
                fs::hard_link(&saved, &path).unwrap();
                path.clone()
            }
            "public" => {
                state::private_file(&path, true).unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
                path.clone()
            }
            "directory" => {
                state::directory(&path).unwrap();
                path.clone()
            }
            "loop" => {
                std::os::unix::fs::symlink(&path, &path).unwrap();
                path.join("record")
            }
            _ => unreachable!(),
        };
        assert!(
            write_json(&target, &json!({"new":true})).is_err(),
            "{defect}"
        );
        assert_eq!(fs::read(saved).unwrap(), OLD);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 2);
    }
}

#[test]
fn target_metadata_errors_keep_their_errno_before_preparation() {
    let (_temp, path) = fixture();
    let invalid = path.with_file_name("x".repeat(4096));
    let error = write_json(&invalid, &json!({"new":true})).unwrap_err();
    assert_eq!(error.io_raw_os_error(), Some(libc::ENAMETOOLONG));
    assert_eq!(fs::read(&path).unwrap(), OLD);
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}
