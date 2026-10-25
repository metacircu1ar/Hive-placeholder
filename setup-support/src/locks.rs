//! Stable path-scoped locks live outside the object that maintenance may remove.
use crate::{Result, files, invalid};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

pub struct Guard(File);
impl Guard {
    pub(crate) fn shared(file: File) -> Result<Self> {
        rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockShared)?;
        Ok(Self(file))
    }

    pub(crate) fn check_identity(&self, path: &Path) -> Result<()> {
        let opened = self.0.metadata()?;
        let named = std::fs::symlink_metadata(path)?;
        if !named.is_file() || (opened.dev(), opened.ino()) != (named.dev(), named.ino()) {
            return Err(invalid("External ownership lock was replaced."));
        }
        Ok(())
    }
    /// Operation-scoped authority; callers validate the opened file first.
    /// Duplicated or inherited descriptors must not extend this guard's lifetime.
    pub fn exclusive(file: File) -> Result<Self> {
        rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)?;
        Ok(Self(file))
    }
    /// Preserve blocking acquisition while releasing logical ownership on drop.
    pub fn blocking_exclusive(file: File) -> Result<Self> {
        file.lock()?;
        Ok(Self(file))
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        loop {
            match rustix::fs::flock(&self.0, rustix::fs::FlockOperation::Unlock) {
                Ok(()) => break,
                Err(rustix::io::Errno::INTR) => (),
                Err(cause) => {
                    let error = std::io::Error::from(cause);
                    tracing::error!(name: "setup.lock_release_failed",
                         error = &error as &(dyn std::error::Error + 'static),
                         decision = "close_descriptor");
                    break;
                }
            }
        }
    }
}

pub fn path(path: &Path, domain: &str, create_parent: bool) -> Result<Guard> {
    path_lock(path, domain, create_parent, false)
}
/// Serialize short initialization/publication operations in the same path domain.
pub fn path_blocking(path: &Path, domain: &str, create_parent: bool) -> Result<Guard> {
    path_lock(path, domain, create_parent, true)
}
pub struct Existing {
    guard: Guard,
    path: PathBuf,
}
impl Existing {
    pub fn check(&self) -> Result<()> {
        self.guard.check_identity(&self.path)
    }
}
/// Zero-publication inspection of an existing external ownership lock.
pub fn existing(path: &Path, domain: &str) -> Result<Existing> {
    let path = location(path, domain, false)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != rustix::process::geteuid().as_raw()
        || meta.mode() & 0o777 != 0o600
        || meta.len() != 0
    {
        return Err(invalid("Unsafe existing operation lock."));
    }
    let existing = Existing {
        guard: Guard::exclusive(file)?,
        path,
    };
    existing.check()?;
    Ok(existing)
}

fn location(path: &Path, domain: &str, create_parent: bool) -> Result<PathBuf> {
    let path = files::absolute(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| invalid("A locked path needs a parent."))?;
    if create_parent && !parent.exists() {
        files::directory(parent)?;
    }
    let metadata = std::fs::metadata(parent)?;
    let uid = rustix::process::geteuid().as_raw();
    if !metadata.is_dir() || !crate::directories::ancestor(metadata.uid(), metadata.mode(), uid) {
        return Err(invalid("External lock has an untrusted parent."));
    }
    let hash = format!("{:x}", Sha256::digest(path.as_os_str().as_encoded_bytes()));
    Ok(parent.join(format!(".{domain}-{hash}.lock")))
}
fn path_lock(path: &Path, domain: &str, create_parent: bool, blocking: bool) -> Result<Guard> {
    let lock_path = location(path, domain, create_parent)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&lock_path)?;
    files::read(&lock_path)?;
    let guard = if blocking {
        Guard::blocking_exclusive(file)
    } else {
        Guard::exclusive(file)
    }
    .map_err(|error| crate::context("Cannot acquire external operation lock", error))?;
    files::sync_parent(&lock_path)?;
    Ok(guard)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn opened(path: &Path) -> File {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .unwrap()
    }
    fn busy(path: &Path) {
        let error = Guard::exclusive(opened(path)).err().unwrap();
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    }
    #[test]
    fn raw_file_control_retains_lock_after_owner_closes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("lock");
        let file = opened(&path);
        file.lock().unwrap();
        let inherited = file.try_clone().unwrap();
        drop(file);
        busy(&path);
        // A parallel fixture may have forked another duplicate. Explicit cleanup
        // avoids depending on that unrelated child's exec or exit timing.
        rustix::fs::flock(&inherited, rustix::fs::FlockOperation::Unlock).unwrap();
        drop(inherited);
        drop(Guard::exclusive(opened(&path)).unwrap());
    }
    #[test]
    fn operation_release_is_independent_of_inherited_descriptors_and_acquisition_policy() {
        for blocking in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("lock");
            let file = opened(&path);
            let inherited = file.try_clone().unwrap();
            let owner = if blocking {
                Guard::blocking_exclusive(file)
            } else {
                Guard::exclusive(file)
            }
            .unwrap();
            busy(&path);
            // A failed acquisition must not release the current owner.
            busy(&path);
            drop(owner);
            let next = Guard::exclusive(opened(&path)).unwrap();
            drop(inherited);
            // The prior duplicate belongs to a different open description.
            busy(&path);
            drop(next);
            drop(Guard::exclusive(opened(&path)).unwrap());
        }
    }
    #[test]
    fn fallible_post_acquisition_work_releases_even_with_a_retained_duplicate() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("lock");
        let file = opened(&path);
        let inherited = file.try_clone().unwrap();
        let failed = (|| -> Result<()> {
            let _guard = Guard::exclusive(file)?;
            Err(std::io::Error::from_raw_os_error(libc::EIO))
        })();
        assert_eq!(failed.unwrap_err().raw_os_error(), Some(libc::EIO));
        let next = Guard::exclusive(opened(&path)).unwrap();
        drop(inherited);
        busy(&path);
        drop(next);
    }
    #[test]
    fn writable_parent_is_refused_and_private_parent_is_fenced() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("knowledge");
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(path(&root, "test", false).is_err());
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let guard = path(&root, "test", false).unwrap();
        assert!(path(&root, "test", false).is_err());
        drop(guard);
        path(&root, "test", false).unwrap();
    }
    #[test]
    fn group_writable_owned_parents_allow_private_creation_and_exclusive_locks() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o775)).unwrap();
        let created = temp.path().join("new/deep");
        if rustix::process::geteuid().is_root() {
            assert!(files::directory(&created).is_err());
            assert!(path(&created, "test", false).is_err());
            return;
        }
        for _ in 0..2 {
            files::directory(&created).unwrap();
            assert_eq!(std::fs::metadata(&created).unwrap().mode() & 0o777, 0o700);
        }
        let target = temp.path().join("record");
        let guard = path(&target, "test", false).unwrap();
        assert_eq!(
            path(&target, "test", false).err().unwrap().kind(),
            std::io::ErrorKind::WouldBlock
        );
        files::write(&target, b"record", false).unwrap();
        assert_eq!(std::fs::metadata(&target).unwrap().mode() & 0o777, 0o600);
        drop(guard);
        drop(path(&target, "test", false).unwrap());
        assert_eq!(
            std::fs::metadata(temp.path()).unwrap().mode() & 0o777,
            0o775
        );
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o660)).unwrap();
        assert!(files::read(&target).is_err());
    }
}
