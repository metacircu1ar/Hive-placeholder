use super::*;
use crate::state;
use std::{
    fs,
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

pub(in crate::runtime) fn boundaries(root: &Path, key: &str, name: &str) -> Vec<(PathBuf, Step)> {
    let directory = root.join(key);
    let target = directory.join(name);
    vec![
        (target.clone(), Step::BeforeRemove),
        (target, Step::AfterRemove),
        (directory.clone(), Step::BeforeDirectorySync),
        (directory.clone(), Step::AfterDirectorySync),
        (directory.clone(), Step::BeforeRemove),
        (directory, Step::AfterRemove),
        (root.into(), Step::BeforeDirectorySync),
        (root.into(), Step::AfterDirectorySync),
    ]
}
fn fixture(root: &Path) -> (String, PathBuf) {
    state::directory(root).unwrap();
    let key = "a".repeat(64);
    let directory = root.join(&key);
    state::directory(&directory).unwrap();
    let target = directory.join("file");
    state::private_file(&target, true)
        .unwrap()
        .write_all(b"snapshot")
        .unwrap();
    (key, target)
}

#[test]
fn every_removal_failure_is_retryable_for_named_and_unindexed_snapshots() {
    for orphan in [false, true] {
        for index in 0..8 {
            let temp = crate::runtime::test_directory();
            let root = temp.path().join("attachments");
            let (key, _) = fixture(&root);
            let (expected, at) = boundaries(&root, &key, "file")[index].clone();
            let hits = Arc::new(AtomicUsize::new(0));
            let observed = hits.clone();
            let remove = || {
                if orphan {
                    remove_orphan(&root, &key)
                } else {
                    remove_snapshot(&root, &key, "file")
                }
            };
            let result = state::sync_tests::with_hook(
                move |path, step| {
                    if path == expected && step == at {
                        observed.fetch_add(1, Ordering::SeqCst);
                        return Err(io::Error::from_raw_os_error(libc::EIO));
                    }
                    Ok(())
                },
                remove,
            );
            assert!(result.is_err(), "orphan={orphan}, boundary={index}");
            assert_eq!(hits.load(Ordering::SeqCst), 1);
            remove().unwrap();
            assert!(!root.join(&key).exists());
        }
    }
}

#[test]
fn absent_key_and_empty_orphan_still_require_their_sync_barriers() {
    for empty in [false, true] {
        let temp = crate::runtime::test_directory();
        let root = temp.path().join("attachments");
        state::directory(&root).unwrap();
        let key = "b".repeat(64);
        if empty {
            state::directory(&root.join(&key)).unwrap();
        }
        let expected = if empty { root.join(&key) } else { root.clone() };
        let hits = Arc::new(AtomicUsize::new(0));
        let observed = hits.clone();
        let result = state::sync_tests::with_hook(
            move |path, step| {
                if path == expected && step == Step::BeforeDirectorySync {
                    observed.fetch_add(1, Ordering::SeqCst);
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
                Ok(())
            },
            || remove_orphan(&root, &key),
        );
        assert!(result.is_err());
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        remove_orphan(&root, &key).unwrap();
    }
}

#[test]
fn unsafe_or_unexpected_contents_are_retained_before_any_deletion() {
    for defect in [
        "extra",
        "wrong_name",
        "public",
        "symlink",
        "hardlink",
        "directory",
        "dangling_key",
    ] {
        let temp = crate::runtime::test_directory();
        let root = temp.path().join("attachments");
        let (key, target) = fixture(&root);
        match defect {
            "extra" => {
                state::private_file(&root.join(&key).join("extra"), true).unwrap();
            }
            "wrong_name" => fs::rename(&target, root.join(&key).join("other")).unwrap(),
            "public" => fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap(),
            "symlink" | "hardlink" => {
                fs::rename(&target, root.join("saved")).unwrap();
                if defect == "symlink" {
                    std::os::unix::fs::symlink(root.join("saved"), &target).unwrap();
                } else {
                    fs::hard_link(root.join("saved"), &target).unwrap();
                }
            }
            "directory" => {
                fs::remove_file(&target).unwrap();
                state::directory(&target).unwrap();
            }
            "dangling_key" => {
                fs::rename(root.join(&key), root.join("saved")).unwrap();
                std::os::unix::fs::symlink(root.join("missing"), root.join(&key)).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(remove_snapshot(&root, &key, "file").is_err(), "{defect}");
        assert!(root.join(&key).symlink_metadata().is_ok());
        if defect == "extra" {
            assert_eq!(fs::read(target).unwrap(), b"snapshot");
        }
    }
}

mod publication;
mod replacement;
