//! Only fixed administration publishers share this namespace and Disk lock.
use super::Disk;
#[cfg(test)]
use super::durable;
use crate::{Error, Result};
use atomic_fs::Step;
use rustix::fs::{AtFlags, Dir, Stat};
use std::{ffi::OsStr, io, os::unix::ffi::OsStrExt, path::Path};

mod directory;
use directory::{Directory, safe_file, same};
#[cfg(test)]
mod tests;

fn unsafe_entry() -> Error {
    Error::unsafe_admin_preparation()
}

impl Disk {
    /// Run once after ordinary admission/recovery, before releasing either lock.
    pub(crate) fn cleanup_preparations(&self) -> Result<()> {
        let root = Directory::root(&self.root, self.root_device)?;
        let admin = root.child(".comb", true)?;
        let heads = admin.child("heads", false)?;
        let receipts = admin.child("receipts", false)?;
        let scope = Scope {
            disk: self,
            root,
            admin,
            heads,
            receipts,
        };
        scope.check()?;
        for (directory, path) in [
            (&scope.admin, self.admin.clone()),
            (&scope.heads, self.admin.join("heads")),
            (&scope.receipts, self.admin.join("receipts")),
        ] {
            scope.sweep(directory, &path)?;
        }
        scope.check()
    }
}

struct Scope<'a> {
    disk: &'a Disk,
    root: Directory,
    admin: Directory,
    heads: Directory,
    receipts: Directory,
}
impl Scope<'_> {
    fn check(&self) -> Result<()> {
        self.root.check_root(&self.disk.root)?;
        self.admin.check_at(&self.root, ".comb")?;
        self.heads.check_at(&self.admin, "heads")?;
        self.receipts.check_at(&self.admin, "receipts")?;
        let held = rustix::fs::fstat(&self.disk.writer_lock.0).map_err(io::Error::from)?;
        safe_file(&held, &self.admin.identity)?;
        let named = self.admin.entry(OsStr::new("lock"))?;
        if !same(&held, &named) {
            return Err(unsafe_entry());
        }
        Ok(())
    }
    fn sweep(&self, directory: &Directory, path: &Path) -> Result<()> {
        for entry in Dir::read_from(&directory.file).map_err(io::Error::from)? {
            let entry = entry.map_err(io::Error::from)?;
            let name = OsStr::from_bytes(entry.file_name().to_bytes());
            if !hive_setup_support::names::comb_admin_preparation_name(name) {
                continue;
            }
            let expected = directory.entry(name)?;
            self.remove(directory, path, name, &expected)?;
        }
        // Also required on an empty retry following a failed unlink barrier.
        boundary(path, Step::BeforeDirectorySync)?;
        self.check()?;
        self.disk.durability.sync_directory(&directory.file)?;
        boundary(path, Step::AfterDirectorySync)?;
        self.check()
    }
    fn remove(
        &self,
        directory: &Directory,
        path: &Path,
        name: &OsStr,
        expected: &Stat,
    ) -> Result<()> {
        let target = path.join(name);
        boundary(&target, Step::BeforeRemove)?;
        self.check()?;
        if !same(expected, &directory.entry(name)?) {
            return Err(unsafe_entry());
        }
        rustix::fs::unlinkat(&directory.file, name, AtFlags::empty()).map_err(io::Error::from)?;
        boundary(&target, Step::AfterRemove)
    }
}

fn boundary(path: &Path, step: Step) -> Result<()> {
    #[cfg(test)]
    let result = durable::checkpoint(path, step);
    #[cfg(not(test))]
    let result = {
        let _ = path;
        atomic_fs::Barriers::observe(&atomic_fs::DefaultBarriers, step)
    };
    result.map_err(Error::from)
}
