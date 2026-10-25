use super::*;
use crate::{
    Comb, ErrorCode,
    disk::{Operation, Recording, System, atomic, publication_hooks as hooks},
};
use std::{
    error::Error as _,
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    sync::Arc,
};

mod isolation;
mod processes;

const SHORT: &str = ".comb-tmp-admin-ABC123";
const LONG: &str = ".comb-tmp-admin-0123456789abcdef";

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    drop(Comb::init(root.path()).unwrap());
    root
}
fn candidate(path: &Path) {
    atomic(path, b"abandoned preparation", &System).unwrap();
}
fn candidates(disk: &Disk) -> Vec<PathBuf> {
    [
        disk.admin.clone(),
        disk.admin.join("heads"),
        disk.admin.join("receipts"),
    ]
    .into_iter()
    .flat_map(|directory| [directory.join(SHORT), directory.join(LONG)])
    .inspect(|path| candidate(path))
    .collect()
}

#[test]
fn successful_open_sweeps_only_exact_names_in_three_fixed_directories() {
    let root = fixture();
    let disk = Disk::acquire(root.path(), false).unwrap();
    let removed = candidates(&disk);
    let mut retained = Vec::new();
    for directory in [
        &disk.admin,
        &disk.admin.join("heads"),
        &disk.admin.join("receipts"),
    ] {
        for name in [
            ".comb-tmp-ABC123",
            ".hive-tmp-state-ABC123",
            ".comb-tmp-admin-ABCDE",
            // Distinct even on case-insensitive macOS volumes.
            ".comb-tmp-admin-1123456789abcdeF",
            "user-export",
        ] {
            let path = directory.join(name);
            candidate(&path);
            retained.push(path);
        }
        let nested = directory.join("nested");
        fs::create_dir(&nested).unwrap();
        candidate(&nested.join(SHORT));
        retained.push(nested.join(SHORT));
    }
    candidate(&disk.root.join(SHORT));
    retained.push(disk.root.join(SHORT));
    let identity = fs::read(disk.admin.join("identity.json")).unwrap();
    drop(disk);
    let comb = Comb::open(root.path()).unwrap();
    assert!(removed.iter().all(|path| !path.exists()));
    assert!(
        retained
            .iter()
            .all(|path| fs::read(path).unwrap() == b"abandoned preparation")
    );
    assert_eq!(
        fs::read(comb.disk.admin.join("identity.json")).unwrap(),
        identity
    );
    assert_eq!(comb.stats().unwrap().pages, 0);
}

#[test]
fn preflight_and_failed_normal_admission_do_not_sweep() {
    for broken in ["identity", "index-mode", "intent"] {
        let root = fixture();
        let disk = Disk::acquire(root.path(), false).unwrap();
        let paths = candidates(&disk);
        drop(disk);
        drop(Disk::acquire(root.path(), false).unwrap());
        assert!(paths.iter().all(|path| path.exists()));
        let admin = root.path().join(".comb");
        let index = admin.join("index.sqlite");
        let permissions = fs::metadata(&index).unwrap().permissions();
        match broken {
            "identity" => atomic(&admin.join("identity.json"), b"{}", &System).unwrap(),
            "index-mode" => fs::set_permissions(&index, fs::Permissions::from_mode(0o620)).unwrap(),
            _ => atomic(&admin.join("intent.json"), b"{}", &System).unwrap(),
        }
        let error = Comb::open(root.path()).err().unwrap();
        if broken == "index-mode" {
            assert_eq!(error.code, ErrorCode::UnsafePath);
            fs::set_permissions(&index, permissions).unwrap();
        }
        assert!(paths.iter().all(|path| path.exists()));
    }
}

#[test]
fn unsafe_candidates_fail_without_deleting_them_or_their_targets() {
    for kind in [
        "symlink",
        "hardlink",
        "directory",
        "fifo",
        "shared-read",
        "shared-write",
    ] {
        let root = fixture();
        let disk = Disk::acquire(root.path(), false).unwrap();
        let path = disk.admin.join(SHORT);
        let outside = root.path().join("kept");
        candidate(&outside);
        match kind {
            "symlink" => symlink(&outside, &path).unwrap(),
            "hardlink" => fs::hard_link(&outside, &path).unwrap(),
            "directory" => fs::create_dir(&path).unwrap(),
            "fifo" => crate::test_support::fifo(&path).unwrap(),
            _ => {
                candidate(&path);
                let mode = if kind == "shared-read" { 0o640 } else { 0o620 };
                fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            }
        }
        assert_eq!(
            disk.cleanup_preparations().unwrap_err().code,
            ErrorCode::UnsafePath
        );
        assert!(fs::symlink_metadata(&path).is_ok());
        assert_eq!(fs::read(&outside).unwrap(), b"abandoned preparation");
    }
}

#[test]
fn changed_lock_directory_or_candidate_refuses_deletion() {
    for kind in ["lock", "directory", "candidate", "mode", "final-lock"] {
        let root = fixture();
        let disk = Disk::acquire(root.path(), false).unwrap();
        let path = disk.admin.join("heads").join(SHORT);
        candidate(&path);
        let target = path.clone();
        let admin = disk.admin.clone();
        let error = hooks::with(
            move |at, step| {
                if (kind != "final-lock" && at == target && step == Step::BeforeRemove)
                    || (kind == "final-lock"
                        && at == admin.join("receipts")
                        && step == Step::AfterDirectorySync)
                {
                    let replaced = match kind {
                        "lock" | "final-lock" => admin.join("lock"),
                        "directory" => admin.join("heads"),
                        _ => target.clone(),
                    };
                    if kind == "mode" {
                        fs::set_permissions(&replaced, fs::Permissions::from_mode(0o640))?;
                    } else {
                        fs::rename(&replaced, replaced.with_extension("kept"))?;
                        if kind == "directory" {
                            fs::create_dir(&replaced)?;
                        } else {
                            candidate(&replaced);
                        }
                    }
                }
                Ok(())
            },
            || disk.cleanup_preparations(),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::UnsafePath, "{kind}");
        if kind == "directory" {
            assert!(disk.admin.join("heads.kept").join(SHORT).exists());
        } else if kind != "final-lock" {
            assert!(path.exists());
        }
    }
}

#[test]
fn failed_unlink_or_barrier_retries_all_three_directory_syncs_even_when_empty() {
    for at in [
        Step::BeforeRemove,
        Step::AfterRemove,
        Step::BeforeDirectorySync,
        Step::AfterDirectorySync,
    ] {
        let root = fixture();
        let recording = Arc::new(Recording::default());
        let disk = Disk::acquire_with(root.path(), false, recording.clone()).unwrap();
        let path = disk.admin.join(SHORT);
        candidate(&path);
        let admin = disk.admin.clone();
        let error = hooks::with(
            move |path, step| {
                if step == at && (path == admin || path == admin.join(SHORT)) {
                    Err(io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
            || disk.cleanup_preparations(),
        )
        .unwrap_err();
        assert_eq!(
            error
                .source()
                .unwrap()
                .downcast_ref::<io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(libc::EIO)
        );
        assert_eq!(path.exists(), at == Step::BeforeRemove);
        recording.reset();
        disk.cleanup_preparations().unwrap();
        assert!(!path.exists());
        assert_eq!(recording.counts().events, [Operation::DirectorySync; 3]);
        recording.reset();
        recording.fail(Operation::DirectorySync, 2);
        assert!(disk.cleanup_preparations().is_err());
        recording.reset();
        disk.cleanup_preparations().unwrap();
        assert_eq!(recording.counts().events, [Operation::DirectorySync; 3]);
    }
}

#[test]
fn owner_and_device_checks_reject_foreign_metadata() {
    let root = fixture();
    let disk = Disk::acquire(root.path(), false).unwrap();
    let directory = Directory::root(&disk.root, disk.root_device).unwrap();
    let mut meta = rustix::fs::fstat(&disk.writer_lock.0).unwrap();
    meta.st_uid ^= 1;
    assert!(safe_file(&meta, &directory.identity).is_err());
    meta.st_uid ^= 1;
    meta.st_dev ^= 1;
    assert!(safe_file(&meta, &directory.identity).is_err());
}

#[test]
fn normal_open_preserves_existing_child_directory_modes() {
    for (leaf, mode, accepted) in [
        ("heads", 0o755, true),
        ("receipts", 0o750, true),
        ("heads", 0o770, !rustix::process::geteuid().is_root()),
        ("receipts", 0o720, !rustix::process::geteuid().is_root()),
        ("heads", 0o777, false),
        ("receipts", 0o722, false),
        ("", 0o750, false),
    ] {
        let root = fixture();
        let admin = root.path().join(".comb");
        let path = admin.join(leaf);
        candidate(&admin.join(SHORT));
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        let opened = Comb::open(root.path());
        assert_eq!(
            opened.is_ok(),
            accepted,
            "{leaf}/{mode:o}: {:?}",
            opened.err().map(|error| error.code)
        );
        assert_eq!(admin.join(SHORT).exists(), !accepted);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            mode
        );
    }
}
