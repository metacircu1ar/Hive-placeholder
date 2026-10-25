//! Verify private image ownership and durable native execution grants.

use super::*;
use super::{
    gate::{PreparedSpawn, complete_launch, prepare_spawn},
    image::{build_image, copy_private_image, sweep_images},
};
use crate::{error::Error, os, state};
use std::{
    fs,
    os::{
        fd::AsRawFd,
        unix::{fs::PermissionsExt, net::UnixStream, process::CommandExt},
    },
    path::PathBuf,
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) fn private_root() -> std::io::Result<tempfile::TempDir> {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
}

pub(crate) fn test_cache() -> crate::error::Result<Arc<LauncherCache>> {
    // A test's CLI child processes can share one private root and so reuse one
    // image; any other test cache gets a fresh root of its own.
    let shared = std::env::var_os("HIVE_TEST_LAUNCH_ROOT").map(PathBuf::from);
    let fresh = match &shared {
        Some(_) => None,
        None => Some(private_root().map_err(Error::state_io_from)?),
    };
    let root = match (&shared, &fresh) {
        (Some(root), _) => root.clone(),
        (None, Some(fresh)) => fresh.path().to_owned(),
        (None, None) => return Err(state::recovery_required()),
    };
    let executable = if let Some(binary) = std::env::var_os("HIVE_NATIVE_SHELL_BINARY") {
        let binary = PathBuf::from(binary)
            .canonicalize()
            .map_err(Error::state_io_from)?;
        if !binary.ends_with("target/release/hivectl") {
            return Err(Error::invalid_executable(
                "Native canaries require the release Hive binary.",
            ));
        }
        binary
    } else {
        std::env::current_exe()
            .map_err(Error::state_io_from)?
            .parent()
            .and_then(std::path::Path::parent)
            .ok_or_else(state::recovery_required)?
            .join("hivectl")
    };
    Ok(Arc::new(LauncherCache::new(
        move || {
            // The fresh root lives as long as its cache.
            let _owned = &fresh;
            Ok(root.clone())
        },
        move || Ok(executable.clone()),
    )))
}
#[test]
fn published_image_is_private_and_reclaimable() {
    let _fork = os::fork_guard().unwrap();
    let root = private_root().unwrap();
    let source = root.path().join("source");
    fs::write(&source, b"executable contents").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
    let directory = root.path().join("hive-launch-interrupted");
    state::directory(&directory).unwrap();
    let owner = state::Lock::acquire(&directory.join("owner.lock")).unwrap();
    let path = directory.join("hive-launch");
    copy_private_image(&source, &path).unwrap();
    assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o500);
    drop(owner);
    let _cache = state::Lock::index(&root.path().join("cache.lock")).unwrap();
    sweep_images(root.path(), None);
    assert!(!directory.exists());
}
#[test]
fn independent_image_builds_do_not_wait_for_an_owned_images_copy() {
    // Independent open descriptions conflict via flock even within one process.
    // Keep unrelated test forks from temporarily retaining either owner's fd.
    let _fork = os::fork_guard().unwrap();
    let root = private_root().unwrap();
    let source = root.path().join("source");
    fs::write(&source, b"trusted launcher fixture").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
    let (ready, held) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let first_root = root.path().to_owned();
    let first_source = source.clone();
    let first = std::thread::spawn(move || {
        build_image(&first_root, &first_source, |path| {
            ready.send(path.to_owned()).unwrap();
            released.recv().unwrap();
        })
    });
    let held = held.recv_timeout(Duration::from_secs(2));
    if held.is_err() {
        let _ = release.send(());
        let _ = first.join();
        panic!("first builder did not reach its owned reservation");
    }
    let first_directory = held.unwrap();
    let (finished, completion) = std::sync::mpsc::channel();
    let second_root = root.path().to_owned();
    let second = std::thread::spawn(move || {
        let result = build_image(&second_root, &source, |_| {});
        let _ = finished.send(());
        result
    });
    let independent = completion.recv_timeout(Duration::from_secs(2)).is_ok();
    let preserved = first_directory.is_dir();
    let owner_held = state::Lock::acquire_existing(&first_directory.join("owner.lock"))
        .is_err_and(|error| matches!(error, Error::ProfileBusy { .. }));
    // The baseline intentionally fails; always release/join both builders first.
    release.send(()).unwrap();
    let first = first.join();
    let second = second.join();
    let first = first.unwrap().unwrap();
    let second = second.unwrap().unwrap();
    assert!(
        independent,
        "independent builder waited behind an owned image's copy"
    );
    assert!(preserved && owner_held);
    for image in [&first, &second] {
        assert_eq!(fs::read(&image.path).unwrap(), b"trusted launcher fixture");
        assert_eq!(
            image.path.metadata().unwrap().permissions().mode() & 0o777,
            0o500
        );
        assert_eq!(
            image.directory.metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}
#[test]
fn cache_gc_preserves_live_or_unproven_owners_and_reclaims_abandoned_images() {
    // Keep unrelated concurrent test forks from temporarily inheriting the owner.
    let _fork = os::fork_guard().unwrap();
    let root = private_root().unwrap();
    let live = root.path().join("hive-launch-live");
    let unknown = root.path().join("hive-launch-unknown");
    for dir in [&live, &unknown] {
        state::directory(dir).unwrap();
        state::private_file(&dir.join("hive-launch"), true).unwrap();
    }
    let owner = state::Lock::acquire(&live.join("owner.lock")).unwrap();
    let _cache = state::Lock::index(&root.path().join("cache.lock")).unwrap();
    sweep_images(root.path(), None);
    assert!(live.exists() && unknown.exists());
    assert!(!unknown.join("owner.lock").exists());
    drop(owner);
    sweep_images(root.path(), None);
    assert!(!live.exists());
    assert!(unknown.exists());
}
#[test]
fn image_creation_is_private_with_a_permissive_cli_umask() {
    if std::env::var_os("HIVE_IMAGE_UMASK_FIXTURE").is_some() {
        // SAFETY: the parent starts this fixture alone in a separate process.
        unsafe {
            libc::umask(0o022);
        }
        let cache = test_cache().unwrap();
        let path = cache.image().unwrap();
        assert_eq!(
            path.parent()
                .unwrap()
                .metadata()
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        return;
    }
    crate::test_support::run_isolated(
        crate::test_support::rerun_isolated(
            "launch::tests::image_creation_is_private_with_a_permissive_cli_umask",
            false,
        )
        .env("HIVE_IMAGE_UMASK_FIXTURE", "1"),
    );
}
#[test]
fn delayed_authorization_cannot_grant_after_its_deadline() {
    let launcher = test_cache().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("native");
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "echo unsafe > \"$1\"", "shim-test"])
        .arg(&marker);
    let PreparedSpawn {
        child,
        parent: gate,
        ..
    } = prepare_spawn(&launcher, &command, true, &[], None).unwrap();
    let identity = state::ProcessIdentity::read(child.id()).unwrap().unwrap();
    let error = complete_launch(child, gate, Instant::now(), |_| Ok(())).unwrap_err();
    assert!(matches!(error, Error::SpawnFailed { .. }));
    assert!(identity.absent().unwrap());
    assert!(!marker.exists());
}
#[test]
fn native_exec_waits_for_durable_authorization_and_preserves_pid() {
    let launcher = test_cache().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("native");
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "echo $$ > \"$1\"", "shim-test"])
        .arg(&marker);
    let mut authorized = None;
    let mut child = spawn(&launcher, &command, true, &[], |pid| {
        assert!(!marker.exists());
        authorized = crate::state::ProcessIdentity::read(pid)?;
        std::thread::sleep(Duration::from_millis(30));
        assert!(!marker.exists());
        Ok(())
    })
    .unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(
        fs::read_to_string(marker)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap(),
        authorized.unwrap().pid
    );
}
#[test]
fn failed_authorization_never_executes_native_code() {
    let launcher = test_cache().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("native");
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "echo unsafe > \"$1\"", "shim-test"])
        .arg(&marker);
    assert!(
        spawn(&launcher, &command, true, &[], |_| Err(
            state::recovery_required()
        ))
        .is_err()
    );
    assert!(!marker.exists());
}
#[test]
fn exec_failure_is_reported_before_provider_admission() {
    let launcher = test_cache().unwrap();
    let command = Command::new("/does-not-exist/hive-native");
    let error = spawn(&launcher, &command, true, &[], |_| Ok(())).unwrap_err();
    assert!(matches!(error, Error::SpawnFailed { .. }));
}
#[test]
fn shim_eof_and_timeout_exit_without_native_execution_or_parent_kill() {
    for eof in [true, false] {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("must-not-run");
        let (parent, gate) = UnixStream::pair().unwrap();
        let fd = gate.as_raw_fd();
        let cache = test_cache().unwrap();
        let mut command = Command::new(cache.image().unwrap());
        command
            .arg(fd.to_string())
            .args(["/bin/sh", "-c", "echo unsafe > \"$1\"", "gate-test"])
            .arg(&marker);
        // SAFETY: only the owned integer FD is changed before exec.
        unsafe {
            command.pre_exec(move || os::inherit_retained_fd(fd));
        }
        let mut child = os::spawn(&mut command).unwrap();
        drop(gate);
        let parent = if eof {
            drop(parent);
            None
        } else {
            Some(parent)
        };
        let started = Instant::now();
        assert_eq!(child.wait().unwrap().code(), Some(126));
        if !eof {
            assert!(started.elapsed() >= Duration::from_secs(29));
        }
        assert!(!marker.exists());
        drop(parent);
    }
}

fn reusable_source(root: &std::path::Path) -> PathBuf {
    let source = root.join("source");
    fs::write(&source, b"reusable launcher fixture").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
    source
}
fn shared_image(root: &std::path::Path, source: &std::path::Path) -> super::image::Image {
    let mut image = build_image(root, source, |_| {}).unwrap();
    image.share().unwrap();
    image
}
fn inode(path: &std::path::Path) -> u64 {
    std::os::unix::fs::MetadataExt::ino(&fs::metadata(path).unwrap())
}
fn sweep_keeping(root: &std::path::Path, reusable: Option<&str>) {
    let _cache = state::Lock::index(&root.join("cache.lock")).unwrap();
    sweep_images(root, reusable);
}
#[test]
fn the_same_executable_reuses_its_checked_image() {
    let _fork = os::fork_guard().unwrap();
    let root = private_root().unwrap();
    let source = reusable_source(root.path());
    let first = shared_image(root.path(), &source);
    let second = build_image(root.path(), &source, |_| panic!("copied again")).unwrap();
    assert_eq!(second.path, first.path);
    assert_eq!(inode(&second.path), inode(&first.path));
    assert_eq!(second.digest, first.digest);
    let name = format!("hive-launch-{}", first.digest);
    assert_eq!(first.directory, root.path().join(&name));
    let path = first.path.clone();
    // Shared holders never remove it, and a sweep keeps an image in use.
    drop(first);
    sweep_keeping(root.path(), None);
    assert!(path.exists());
    drop(second);
    // Unheld, it stays for its own executable and goes for any other.
    sweep_keeping(root.path(), Some(&name));
    assert!(path.exists());
    sweep_keeping(root.path(), None);
    assert!(!path.exists());
}
#[test]
fn an_exclusive_builder_never_makes_another_wait() {
    let _fork = os::fork_guard().unwrap();
    let root = private_root().unwrap();
    let source = reusable_source(root.path());
    // Not yet self-checked, so still held exclusively.
    let first = build_image(root.path(), &source, |_| {}).unwrap();
    let second = build_image(root.path(), &source, |_| {}).unwrap();
    assert_ne!(second.directory, first.directory);
    assert_eq!(second.digest, first.digest);
}
#[test]
fn a_changed_cached_image_is_never_reused() {
    let _fork = os::fork_guard().unwrap();
    for defect in ["mode", "content", "extra", "link", "symlink"] {
        let root = private_root().unwrap();
        let source = reusable_source(root.path());
        let first = shared_image(root.path(), &source);
        let (directory, path) = (first.directory.clone(), first.path.clone());
        drop(first);
        match defect {
            "mode" => fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap(),
            "content" => {
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
                fs::write(&path, b"another launcher").unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o500)).unwrap();
            }
            "extra" => fs::write(directory.join("extra"), b"unknown").unwrap(),
            "link" => fs::hard_link(&path, root.path().join("linked")).unwrap(),
            _ => {
                fs::rename(&path, root.path().join("moved")).unwrap();
                std::os::unix::fs::symlink(root.path().join("moved"), &path).unwrap();
            }
        }
        let rebuilt = build_image(root.path(), &source, |_| {}).unwrap();
        // An unheld image whose own files are intact is removed and copied
        // anew under its name; one with unknown or shared entries is left for
        // inspection, never deleted, and a separate image is built. A new copy
        // can reuse the old inode number on Linux, so check the repaired file.
        if matches!(defect, "mode" | "content") {
            assert_eq!(rebuilt.directory, directory, "{defect}");
            let mode = std::os::unix::fs::MetadataExt::mode(&fs::metadata(&rebuilt.path).unwrap());
            assert_eq!(mode & 0o7777, os::READ_EXECUTE_MODE, "{defect}");
            assert_eq!(fs::read(&rebuilt.path).unwrap(), fs::read(&source).unwrap());
        } else {
            assert_ne!(rebuilt.directory, directory, "{defect}");
            assert!(directory.exists(), "{defect}");
        }
    }
}
#[test]
fn a_source_replaced_during_the_copy_is_never_published_under_its_old_hash() {
    let _fork = os::fork_guard().unwrap();
    let root = private_root().unwrap();
    let source = reusable_source(root.path());
    let old = crate::executable::sha256(&source).unwrap();
    let replaced = source.clone();
    let image = build_image(root.path(), &source, move |_| {
        fs::write(&replaced, b"an upgraded launcher").unwrap();
    })
    .unwrap();
    assert_eq!(image.digest, crate::executable::sha256(&source).unwrap());
    assert_ne!(
        image.directory,
        root.path().join(format!("hive-launch-{old}"))
    );
    assert!(!root.path().join(format!("hive-launch-{old}")).exists());
}
#[test]
fn a_reused_image_that_cannot_run_is_rejected() {
    let fork = os::fork_guard().unwrap();
    let root = private_root().unwrap();
    // Plain data: the self-check cannot execute it.
    let source = reusable_source(root.path());
    let first = shared_image(root.path(), &source);
    let directory = first.directory.clone();
    drop(first);
    let (cache_root, executable) = (root.path().to_owned(), source.clone());
    let cache = LauncherCache::new(
        move || Ok(cache_root.clone()),
        move || Ok(executable.clone()),
    );
    drop(fork);
    assert!(cache.initialize().is_err());
    assert!(!directory.exists());
}
#[test]
fn a_reservation_left_by_a_dead_builder_does_not_block_reuse() {
    let _fork = os::fork_guard().unwrap();
    let root = private_root().unwrap();
    let source = reusable_source(root.path());
    let name = format!(
        "hive-launch-{}",
        crate::executable::sha256(&source).unwrap()
    );
    // A builder died after reserving the name, before creating owner.lock.
    state::directory(&root.path().join(&name)).unwrap();
    let image = build_image(root.path(), &source, |_| {}).unwrap();
    assert_eq!(image.directory, root.path().join(&name));
}
