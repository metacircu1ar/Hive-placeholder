//! A one-file snapshot has an atomic unlink; the SQL outbox owns retry authority.
use super::layout::{Directory, boundary, exists, invalid};
use crate::error::{Error, Result};
use atomic_fs::Step;
use rustix::fs::{AtFlags, unlinkat};
use std::path::Path;

pub(in crate::runtime) fn remove_snapshot(root: &Path, key: &str, name: &str) -> Result<()> {
    super::snapshot_path(root, key, name)?;
    remove(root, key, Some(name))
}
/// Caller proved no SQL blob owns this key under exclusive startup/offline ownership.
pub(in crate::runtime) fn remove_orphan(root: &Path, key: &str) -> Result<()> {
    if !super::valid_key(key) {
        return Err(invalid());
    }
    remove(root, key, None)
}
fn remove(root: &Path, key: &str, name: Option<&str>) -> Result<()> {
    let root = Directory::open(root)?;
    let path = root.path.join(key);
    if let Some(directory) = root.child(key)? {
        let file = directory.entry(name)?;
        if let Some(payload) = &file {
            let target = directory.path.join(&payload.name);
            boundary(&target, Step::BeforeRemove)?;
            root.check()?;
            directory.check_entry(file.as_ref())?;
            unlinkat(&directory.file, &payload.name, AtFlags::empty())
                .map_err(|e| Error::state_io_from(e.into()))?;
            boundary(&target, Step::AfterRemove)?;
        }
        // Repeat on an empty-directory retry before erasing the directory itself.
        root.check()?;
        directory.sync()?;
        boundary(&path, Step::BeforeRemove)?;
        root.check()?;
        directory.check_entry(None)?;
        unlinkat(&root.file, key, AtFlags::REMOVEDIR)
            .map_err(|e| Error::state_io_from(e.into()))?;
        boundary(&path, Step::AfterRemove)?;
    }
    // Absence before this barrier cannot acknowledge a previously failed sync.
    root.sync()?;
    if exists(&path)? {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests;
