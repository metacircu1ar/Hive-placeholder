//! Recovery repeats both sides of page publication before retiring its intent.
use super::{Durability, Error, Result, resync::same_name};
use crate::{
    disk::Disk,
    values::{ContentHash, Slug},
};
use std::{
    fs::{File, OpenOptions},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

impl Disk {
    pub(in crate::disk) fn sync_page_state(
        &self,
        slug: &Slug,
        expected: Option<&ContentHash>,
    ) -> Result<()> {
        let path = self.page_path(slug, false)?;
        let destination = recovery_parent(&path, &self.root, expected.is_none())?;
        let source = open(&self.admin, libc::O_DIRECTORY)?;
        let parent = open(destination, libc::O_DIRECTORY)?;
        let file = expected
            .map(|_| open(&path, libc::O_NONBLOCK))
            .transpose()?;
        let check = || {
            same_name(&source, &self.admin)?;
            same_name(&parent, destination)?;
            if let Some(file) = &file {
                super::check_file(&file.metadata()?)?;
                same_name(file, &path)?;
            }
            if self.content(slug)?.map(ContentHash::digest).as_ref() != expected {
                return Err(Error::interrupted_page_changed());
            }
            Ok(())
        };
        check()?;
        if let Some(file) = &file {
            #[cfg(test)]
            super::checkpoint(&path, atomic_fs::Step::BeforeFileSync)?;
            check()?;
            self.durability.fsync(file)?;
            #[cfg(test)]
            super::checkpoint(&path, atomic_fs::Step::AfterFileSync)?;
            check()?;
        }
        // An absent page still needs the unlink's destination-parent barrier.
        // Rollback repeats the source barrier too: an orphan may remain.
        sync(&source, &path, self.durability.as_ref(), true, &check)?;
        sync(&parent, &path, self.durability.as_ref(), false, &check)?;
        check()
    }
}
fn open(path: &Path, flags: i32) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | flags)
        .open(path)
        .map_err(Error::from)
}
fn sync(
    file: &File,
    page: &Path,
    durability: &dyn Durability,
    source: bool,
    check: &dyn Fn() -> Result<()>,
) -> Result<()> {
    #[cfg(not(test))]
    let _ = (page, source);
    #[cfg(test)]
    super::checkpoint(
        page,
        if source {
            atomic_fs::Step::BeforeSourceDirectorySync
        } else {
            atomic_fs::Step::BeforeDirectorySync
        },
    )?;
    check()?;
    durability.fsync(file)?;
    #[cfg(test)]
    super::checkpoint(
        page,
        if source {
            atomic_fs::Step::AfterSourceDirectorySync
        } else {
            atomic_fs::Step::AfterDirectorySync
        },
    )?;
    check()
}

// A rolled-back creation may have no page directory at all. Sync the nearest
// admitted ancestor that records that absence without creating a directory.
fn recovery_parent<'a>(path: &'a Path, root: &Path, absent: bool) -> Result<&'a Path> {
    let mut parent = path.parent().ok_or_else(Error::parent_missing)?;
    while absent && parent != root {
        match std::fs::symlink_metadata(parent) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                parent = parent.parent().ok_or_else(Error::parent_missing)?;
            }
            Err(error) => return Err(error.into()),
            Ok(_) => break,
        }
    }
    Ok(parent)
}
