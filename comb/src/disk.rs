//! Exclusive ownership, private administration files and recoverable file-first writes.
mod cleanup;
mod durable;
#[path = "disk/fs.rs"]
mod filesystem;
mod importing;
mod initialization;
mod journal;
pub(crate) use journal::{ConsolidationIntent, PreparedPage, Recovery};
mod limits;
mod migration;
pub use migration::UpgradeReport;
pub(crate) use migration::upgrade;
mod load;
mod rebuilding;
mod scan;
mod seal;
mod sidecars;

#[cfg(test)]
pub(crate) use durable::hooks as publication_hooks;
#[cfg(test)]
pub(crate) use durable::{Counts, Operation, Recording};
pub(crate) use durable::{Durability, System, atomic, atomic_admin};
pub(crate) use durable::{create_once, sync_publication_parent};
use filesystem::create_root;
pub(crate) use filesystem::{check_file, dir, exists, read};
#[cfg(test)]
use journal::POPULATION;
use limits::{IDENTITY_BYTES, RECORD_BYTES};
pub(crate) use load::Loaded;
pub(crate) use scan::Pages;
use seal::Identity;
#[cfg(test)]
use seal::service_key;

use crate::{
    Error, Page, Result,
    error::CommitExt,
    values::{CombId, ContentHash, Slug},
};
#[cfg(test)]
use crate::{
    page,
    values::{Principal, RequestId},
};
use serde::Deserialize;
use std::{
    fs::{self, File},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(test)]
pub(crate) mod golden;
#[cfg(test)]
mod values_tests;

fn required_option<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(
    input: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::deserialize(input)
}

pub(crate) struct Disk {
    pub root: PathBuf,
    pub admin: PathBuf,
    root_device: u64,
    identity: Identity,
    durability: Arc<dyn Durability>,
    writer_lock: OwnedLock,
    scope_lock: Option<hive_setup_support::locks::Guard>,
}

// Closing our descriptor alone leaves flock held by a forked child's inherited
// descriptor until exec. Ownership ends here, even if that child has not run yet.
struct OwnedLock(File);

#[cfg(test)]
thread_local! {
    /// When armed, `open` keeps a duplicate of its lock descriptor here, as a
    /// child forked by another thread would hold it.
    static INHERIT_LOCK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static INHERITED_LOCK: std::cell::RefCell<Option<File>> = const { std::cell::RefCell::new(None) };
}

impl Drop for OwnedLock {
    fn drop(&mut self) {
        while rustix::fs::flock(&self.0, rustix::fs::FlockOperation::Unlock)
            == Err(rustix::io::Errno::INTR)
        {}
    }
}

impl Disk {
    pub fn acquire(root: &Path, initialize: bool) -> Result<Self> {
        Self::acquire_with(root, initialize, Arc::new(System))
    }
    fn acquire_with(
        root: &Path,
        initialize: bool,
        durability: Arc<dyn Durability>,
    ) -> Result<Self> {
        let admin = root.join(".comb");
        if exists(&admin)? {
            initialization::check_format(&admin)?;
        }
        if initialize && let Some(parent) = root.parent().filter(|p| !p.as_os_str().is_empty()) {
            create_root(parent, durability.as_ref())?;
        }
        let scope_lock =
            hive_setup_support::locks::path(root, "comb-root", initialize).map_err(|e| {
                if e.kind() == std::io::ErrorKind::WouldBlock {
                    Error::already_open().caused_by(e)
                } else {
                    Error::from(e)
                }
            })?;
        if initialize && !root.exists() {
            create_root(root, durability.as_ref())?;
        }
        dir(root, false, durability.as_ref())?;
        let root = root.canonicalize()?;
        let admin = root.join(".comb");
        let device = fs::metadata(&root)?.dev();
        let fresh = !exists(&admin)?;
        if initialize && fresh {
            initialization::create(&admin, durability.as_ref())?;
        }
        let opened = initialization::open(&admin, device, durability.as_ref());
        let (lock, identity) = if initialize && fresh {
            opened.committed()?
        } else {
            opened?
        };
        initialization::admit_writes(&admin, &lock, durability.as_ref())?;
        if !fresh {
            initialization::finish_parent(&root, durability.as_ref())?;
        }
        Ok(Self {
            root,
            admin,
            root_device: device,
            identity,
            durability,
            writer_lock: lock,
            scope_lock: Some(scope_lock),
        })
    }
    pub(crate) fn take_scope_lock(&mut self) -> Result<hive_setup_support::locks::Guard> {
        self.scope_lock.take().ok_or_else(Error::already_open)
    }
    pub fn durability(&self) -> Arc<dyn Durability> {
        Arc::clone(&self.durability)
    }
    #[cfg(test)]
    pub fn set_durability(&mut self, durability: Arc<dyn Durability>) {
        self.durability = durability;
    }
    pub fn index_path(&self) -> PathBuf {
        self.admin.join("index.sqlite")
    }
    pub fn pages(&self) -> Pages {
        Pages::new(self.root.clone(), self.root_device)
    }
    pub fn id(&self) -> CombId {
        self.identity.id.clone()
    }
}

#[cfg(test)]
mod special_file_tests {
    #[test]
    fn dropping_owner_unlocks_even_when_a_fork_inherits_the_descriptor() {
        let directory = tempfile::tempdir().unwrap();
        let owner = super::Disk::acquire(directory.path(), true).unwrap();
        // dup and fork share the same open file description and flock. Keep that
        // description alive deterministically, without racing a child before exec.
        let inherited = owner.writer_lock.0.try_clone().unwrap();
        assert!(super::Disk::acquire(directory.path(), false).is_err());
        // A failed contender must not unlock the live owner's shared description.
        assert!(super::Disk::acquire(directory.path(), false).is_err());
        drop(owner);
        let reopened = super::Disk::acquire(directory.path(), false);
        drop(inherited);
        assert!(reopened.is_ok(), "{}", reopened.err().unwrap());
    }

    #[test]
    fn a_failed_open_unlocks_even_when_a_fork_inherits_the_descriptor() {
        let directory = tempfile::tempdir().unwrap();
        let aside = tempfile::tempdir().unwrap();
        drop(super::Disk::acquire(directory.path(), true).unwrap());
        // Without heads, opening fails after taking the writer lock.
        let heads = directory.path().join(".comb/heads");
        std::fs::rename(&heads, aside.path().join("heads")).unwrap();
        super::INHERIT_LOCK.set(true);
        let failed = super::Disk::acquire(directory.path(), false);
        super::INHERIT_LOCK.set(false);
        assert!(failed.is_err());
        let inherited = super::INHERITED_LOCK
            .take()
            .expect("the failed open took the writer lock");
        std::fs::rename(aside.path().join("heads"), &heads).unwrap();
        let reopened = super::Disk::acquire(directory.path(), false);
        drop(inherited);
        assert!(reopened.is_ok(), "{}", reopened.err().unwrap());
    }

    #[test]
    fn fifo_is_rejected_without_waiting_for_a_writer() {
        use std::os::unix::fs::FileTypeExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("credential.json");
        crate::test_support::fifo(&path).unwrap();
        assert!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_fifo()
        );
        let start = std::time::Instant::now();
        assert_eq!(
            super::read(&path, 16384).unwrap_err().code,
            crate::ErrorCode::UnsafePath
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }
}
