//! Root-wide writer admission. Nested operations share one process-local owner;
//! migration takes the exclusive side before any lower-level ownership locks.
use crate::{Result, files, invalid, locks::Guard};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::OpenOptions,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

struct Owner {
    guard: Guard,
    path: PathBuf,
    settled: Mutex<Option<(PathBuf, u64, u64)>>,
}

#[derive(Clone)]
pub struct Shared(Arc<Owner>);

impl Shared {
    pub fn acquire(root: &Path, domain: &'static str) -> Result<Self> {
        static OWNERS: OnceLock<Mutex<HashMap<PathBuf, Weak<Owner>>>> = OnceLock::new();
        let path = location(root, domain, true)?;
        let mut owners = OWNERS
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|_| invalid("Root admission registry is unavailable."))?;
        if let Some(owner) = owners.get(&path).and_then(Weak::upgrade) {
            owner.guard.check_identity(&owner.path)?;
            return Ok(Self(owner));
        }
        // Failed/acquired gates never accumulate once their last owner is gone.
        owners.retain(|_, owner| owner.strong_count() != 0);
        let guard = acquire(&path, false, true)?;
        let owner = Arc::new(Owner {
            guard,
            path: path.clone(),
            settled: Mutex::default(),
        });
        owners.insert(path, Arc::downgrade(&owner));
        Ok(Self(owner))
    }

    pub fn check(&self) -> Result<()> {
        self.0.guard.check_identity(&self.0.path)
    }

    pub fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// Settle a removed migration intent once while this exact owner and root
    /// remain held. Failures are retryable; no success survives the last owner.
    pub fn admit_writes(&self, root: &Path, barriers: &impl atomic_fs::Barriers) -> Result<()> {
        self.check()?;
        let mut settled = self
            .0
            .settled
            .lock()
            .map_err(|_| invalid("Migration admission state is unavailable."))?;
        if files::optional_metadata(&root.join(crate::migration::PENDING))?.is_some() {
            return Err(invalid("Recover the pending migration before writing."));
        }
        let identity =
            files::optional_metadata(root)?.map(|meta| (root.to_owned(), meta.dev(), meta.ino()));
        if identity.is_some() && *settled == identity {
            return self.check();
        }
        crate::migration::admit_writes(root, &|| self.check(), barriers)?;
        let after =
            files::optional_metadata(root)?.map(|meta| (root.to_owned(), meta.dev(), meta.ino()));
        if identity != after {
            return Err(invalid("Migration admission root changed."));
        }
        *settled = identity;
        Ok(())
    }
}

/// Does not imply native descendants are gone: product-specific lease/process
/// evidence must also be validated while this owner remains held.
pub struct Exclusive {
    owner: Owner,
}

impl Exclusive {
    /// Existing locks only. Inspection and dry-run must never create an object.
    pub fn existing(root: &Path, domain: &'static str) -> Result<Self> {
        let path = location(root, domain, false)?;
        let guard = acquire(&path, true, false)?;
        Ok(Self {
            owner: Owner {
                guard,
                path,
                settled: Mutex::default(),
            },
        })
    }
    pub fn check(&self) -> Result<()> {
        self.owner.guard.check_identity(&self.owner.path)
    }
}

fn location(root: &Path, domain: &str, create_parent: bool) -> Result<PathBuf> {
    if domain.is_empty()
        || !domain
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
    {
        return Err(invalid("Invalid admission lock domain."));
    }
    let root = files::absolute(root)?;
    let parent = root
        .parent()
        .ok_or_else(|| invalid("A root needs a parent."))?;
    if create_parent && !parent.try_exists()? {
        files::directory(parent)?;
    }
    let metadata = std::fs::metadata(parent)?;
    if !metadata.is_dir()
        || !crate::directories::ancestor(
            metadata.uid(),
            metadata.mode(),
            rustix::process::geteuid().as_raw(),
        )
    {
        return Err(invalid("Root admission has an untrusted parent."));
    }
    let hash = format!("{:x}", Sha256::digest(root.as_os_str().as_encoded_bytes()));
    Ok(parent.join(format!(".{domain}-admission-{hash}.lock")))
}

fn acquire(path: &Path, exclusive: bool, create: bool) -> Result<Guard> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o777 != 0o600
        || metadata.nlink() != 1
        || metadata.len() != 0
    {
        return Err(invalid("Root admission lock is not a private empty file."));
    }
    let guard = if exclusive {
        Guard::exclusive(file)?
    } else {
        Guard::shared(file)?
    };
    guard.check_identity(path)?;
    if create {
        files::sync_parent(path)?;
    }
    Ok(guard)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod settlement_tests;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn nested_and_detached_owners_exclude_upgrade_until_the_last_owner_leaves() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let first = Shared::acquire(&root, "test").unwrap();
        let nested = Shared::acquire(&root, "test").unwrap();
        assert!(Arc::ptr_eq(&first.0, &nested.0));
        let detached = nested.clone();
        drop(first);
        drop(nested);
        assert_eq!(
            Exclusive::existing(&root, "test").err().unwrap().kind(),
            std::io::ErrorKind::WouldBlock
        );
        drop(detached);
        let exclusive = Exclusive::existing(&root, "test").unwrap();
        assert!(Shared::acquire(&root, "test").is_err());
        assert!(Exclusive::existing(&root, "test").is_err());
        drop(exclusive);
        drop(Shared::acquire(&root, "test").unwrap());
    }

    #[test]
    fn inspection_is_zero_write_and_replaced_lock_is_never_reused() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        assert_eq!(
            Exclusive::existing(&root, "test").err().unwrap().kind(),
            std::io::ErrorKind::NotFound
        );
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
        let shared = Shared::acquire(&root, "test").unwrap();
        let path = shared.0.path.clone();
        std::fs::rename(&path, path.with_extension("retained")).unwrap();
        assert!(shared.check().is_err());
        assert!(Shared::acquire(&root, "test").is_err());
        assert!(!path.exists());
    }
}
