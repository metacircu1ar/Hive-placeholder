//! Repeat barriers for existing non-SQLite files without replacing their identity.
use super::{Durability, Error, Result, check_file, dir};
use crate::disk::filesystem::check_name;
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

/// Retry of a direct administration record must keep the private-parent contract.
pub(in crate::disk) fn sync_admin(path: &Path, durability: &dyn Durability) -> Result<()> {
    super::restricted_parent(path, durability, 0o077)?;
    sync_existing(path, durability, true)?;
    super::restricted_parent(path, durability, 0o077)
}

pub(in crate::disk) fn sync_existing(
    path: &Path,
    durability: &dyn Durability,
    full: bool,
) -> Result<()> {
    let parent = path.parent().ok_or_else(Error::parent_missing)?;
    dir(parent, false, durability)?;
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(parent)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    check_file(&file.metadata()?)?;
    check_name(&file, path)?;
    same_name(&directory, parent)?;
    same_name(&file, path)?;
    #[cfg(test)]
    super::checkpoint(path, atomic_fs::Step::BeforeFileSync)?;
    durability.fsync(&file)?;
    #[cfg(test)]
    super::checkpoint(path, atomic_fs::Step::AfterFileSync)?;
    #[cfg(test)]
    super::checkpoint(path, atomic_fs::Step::BeforeDirectorySync)?;
    durability.fsync(&directory)?;
    #[cfg(test)]
    super::checkpoint(path, atomic_fs::Step::AfterDirectorySync)?;
    if full {
        durability.full_flush(&file)?;
    }
    same_name(&directory, parent)?;
    same_name(&file, path)
}

pub(super) fn same_name(file: &File, path: &Path) -> Result<()> {
    let held = file.metadata()?;
    let named = fs::symlink_metadata(path)?;
    if named.file_type().is_symlink() || (held.dev(), held.ino()) != (named.dev(), named.ino()) {
        return Err(Error::unsafe_file());
    }
    Ok(())
}
