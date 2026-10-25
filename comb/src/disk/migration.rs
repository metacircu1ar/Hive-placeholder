//! Existing-only upgrade admission; never enters normal intent recovery or opens
//! original SQLite during an inspection. The released adapter registry is empty.
use super::{Disk, System, dir, initialization};
use crate::{Error, Result, index::Index};
use hive_setup_support::{
    locks,
    migration::{Backup, Engine, Receipt},
};
use std::{fs, os::unix::fs::MetadataExt, path::Path, sync::Arc};

#[derive(serde::Serialize)]
pub struct UpgradeReport {
    pub format: u32,
    pub dry_run: bool,
    pub status: &'static str,
    pub steps: Vec<(u32, u32)>,
    pub migrations: Vec<Receipt>,
    pub backups: Vec<Backup>,
}

struct Owner {
    disk: Disk,
    scope: locks::Existing,
    root_inode: u64,
    admin_inode: u64,
}
impl Owner {
    fn acquire(root: &Path) -> Result<Self> {
        let root = hive_setup_support::files::absolute(root)?;
        dir(&root, false, &System)?;
        let scope = locks::existing(&root, "comb-root")?;
        let root_meta = fs::symlink_metadata(&root)?;
        let admin = root.join(".comb");
        let (writer_lock, identity) = initialization::open(&admin, root_meta.dev(), &System)?;
        let admin_inode = fs::symlink_metadata(&admin)?.ino();
        let disk = Disk {
            root,
            admin,
            root_device: root_meta.dev(),
            identity,
            durability: Arc::new(System),
            writer_lock,
            scope_lock: None,
        };
        let owner = Self {
            disk,
            scope,
            root_inode: root_meta.ino(),
            admin_inode,
        };
        owner.check()?;
        Ok(owner)
    }
    fn check(&self) -> Result<()> {
        self.scope.check()?;
        for (path, inode) in [
            (&self.disk.root, self.root_inode),
            (&self.disk.admin, self.admin_inode),
        ] {
            dir(path, false, &System)?;
            let meta = fs::symlink_metadata(path)?;
            if (meta.dev(), meta.ino()) != (self.disk.root_device, inode) {
                return Err(Error::unsafe_directory());
            }
        }
        let held = self.disk.writer_lock.0.metadata()?;
        let named = fs::symlink_metadata(self.disk.admin.join("lock"))?;
        if !named.is_file() || (held.dev(), held.ino()) != (named.dev(), named.ino()) {
            return Err(Error::unsafe_lock());
        }
        Ok(())
    }
}

pub(crate) fn upgrade(root: &Path, dry_run: bool) -> Result<UpgradeReport> {
    let owner = Owner::acquire(root)?;
    let check = || owner.check().map_err(std::io::Error::other);
    let disk = &owner.disk;
    let engine = Engine::new(&disk.root, &disk.admin, Path::new(".comb/format"), &check)?;
    if engine.pending()?.is_some() {
        return Err(Error::pending_upgrade());
    }
    if super::exists(&disk.admin.join("intent.json"))? {
        return Err(Error::pending_mutation());
    }
    disk.reject_pending_rebuild()?;
    Index::inspect_migration(&disk.index_path())?;
    let report = UpgradeReport {
        format: crate::compatibility::ROOT.current,
        dry_run,
        status: "current",
        steps: Vec::new(),
        migrations: engine.history()?,
        backups: engine.backups()?,
    };
    owner.check()?;
    Ok(report)
}

#[cfg(test)]
mod seals;
#[cfg(test)]
mod tests;
