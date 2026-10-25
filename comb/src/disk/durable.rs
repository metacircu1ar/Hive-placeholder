//! Ordered replacement and the existing coalesced device-cache barriers.
#[cfg(target_os = "macos")]
mod os;
mod page;
mod resync;
pub(super) use resync::{sync_admin, sync_existing};
#[cfg(test)]
mod recording;
#[cfg(test)]
mod tests;

use super::{check_file, dir};
use crate::{Error, Result, error::CommitExt};
#[cfg(test)]
pub(crate) use recording::{Counts, Operation, Recording};
use std::{
    fs::{self, File},
    io,
    path::Path,
};

pub(crate) trait Durability: Send + Sync {
    fn fsync(&self, file: &File) -> io::Result<()>;
    #[cfg(any(target_os = "macos", test))]
    fn ordering_barrier(&self, file: &File) -> io::Result<()>;
    fn full_flush(&self, file: &File) -> io::Result<()>;
    fn sync_directory(&self, file: &File) -> io::Result<()>;
    fn before_finish(&self) -> Result<()> {
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct System;
impl Durability for System {
    fn fsync(&self, file: &File) -> io::Result<()> {
        rustix::fs::fsync(file).map_err(io::Error::from)
    }
    #[cfg(any(target_os = "macos", test))]
    fn ordering_barrier(&self, file: &File) -> io::Result<()> {
        #[cfg(target_os = "macos")]
        {
            os::ordering_barrier(file)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = file;
            Ok(())
        }
    }
    fn full_flush(&self, file: &File) -> io::Result<()> {
        file.sync_all()
    }
    fn sync_directory(&self, file: &File) -> io::Result<()> {
        file.sync_all()
    }
}

pub(crate) fn atomic(path: &Path, bytes: &[u8], durability: &dyn Durability) -> Result<()> {
    replace(path, bytes, durability, true, None, ".comb-tmp-")
}

/// Fixed administration records published while the Disk writer is held.
pub(crate) fn atomic_admin(path: &Path, bytes: &[u8], durability: &dyn Durability) -> Result<()> {
    let check = || restricted_parent(path, durability, 0o077);
    check()?;
    replace(
        path,
        bytes,
        durability,
        true,
        Some(&check),
        hive_setup_support::names::COMB_ADMIN_PREFIX,
    )
}

/// Authoritative records require private administration; standalone index helpers
/// retain their existing no-shared-write rule. Neither uses page-directory policy.
fn restricted_parent(path: &Path, durability: &dyn Durability, forbidden: u32) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let parent = path.parent().ok_or_else(Error::parent_directory_missing)?;
    dir(parent, false, durability)?;
    if fs::symlink_metadata(parent)?.mode() & forbidden != 0 {
        return Err(Error::unsafe_directory());
    }
    Ok(())
}

/// Publish a complete derived database before SQLite opens its final name.
pub(crate) fn create_once(path: &Path, bytes: &[u8]) -> Result<()> {
    let check = || restricted_parent(path, &System, 0o022);
    check()?;
    atomic_fs::PreparedCreate::new(
        path,
        (hive_setup_support::names::PUBLICATION_OPTIONS)
            .with_prefix(hive_setup_support::names::COMB_ADMIN_PREFIX),
    )
    .and_then(|prepared| {
        prepared.publish_with(
            bytes,
            &Publication {
                #[cfg(test)]
                path,
                durability: &System,
                finish: true,
                check: Some(&check),
            },
        )
    })
    .map_err(publication)
}

/// Publish a previously absent page without replacing an editor's new file.
pub(super) fn create_ordered_checked(
    path: &Path,
    preparation_directory: &Path,
    bytes: &[u8],
    durability: &dyn Durability,
    check: &dyn Fn() -> Result<()>,
) -> Result<()> {
    dir(
        path.parent().ok_or_else(Error::parent_directory_missing)?,
        false,
        durability,
    )?;
    atomic_fs::PreparedCreate::new_in(
        path,
        preparation_directory,
        (hive_setup_support::names::PUBLICATION_OPTIONS)
            .with_prefix(hive_setup_support::names::COMB_ADMIN_PREFIX),
    )
    .and_then(|prepared| {
        prepared.publish_with(
            bytes,
            &Publication {
                #[cfg(test)]
                path,
                durability,
                finish: false,
                check: Some(check),
            },
        )
    })
    .map_err(|cause| {
        if cause.io_error().kind() == io::ErrorKind::AlreadyExists {
            // Ordered creation follows durable intent acceptance. A creator can
            // win before preparation or after the last content check.
            let mut error = Error::interrupted_page_changed();
            error.file_committed = cause.published();
            error.caused_by(cause)
        } else {
            publication(cause)
        }
    })
}

pub(crate) fn sync_publication_parent(path: &Path) -> Result<()> {
    let parent = path.parent().ok_or_else(Error::parent_directory_missing)?;
    restricted_parent(path, &System, 0o022)?;
    #[cfg(test)]
    checkpoint(path, atomic_fs::Step::BeforeDirectorySync).committed()?;
    restricted_parent(path, &System, 0o022).committed()?;
    System.fsync(&File::open(parent).committed()?).committed()?;
    #[cfg(test)]
    checkpoint(path, atomic_fs::Step::AfterDirectorySync).committed()?;
    Ok(())
}

pub(super) fn replace_ordered(
    path: &Path,
    bytes: &[u8],
    durability: &dyn Durability,
) -> Result<()> {
    replace(
        path,
        bytes,
        durability,
        false,
        None,
        hive_setup_support::names::COMB_ADMIN_PREFIX,
    )
}

/// Sync each file and directory, but coalesce device-cache barriers at the
/// intent and completion boundaries. This matters on macOS, where `sync_all` on
/// regular files requests `F_FULLFSYNC`; ordinary fsync alone leaves drive caches.
pub(super) fn replace_checked(
    path: &Path,
    preparation_directory: &Path,
    bytes: &[u8],
    durability: &dyn Durability,
    check: &dyn Fn() -> Result<()>,
) -> Result<()> {
    check_destination(path, durability)?;
    atomic_fs::Publisher::new(
        (hive_setup_support::names::PUBLICATION_OPTIONS)
            .with_prefix(hive_setup_support::names::COMB_ADMIN_PREFIX),
    )
    .with_barriers(&Publication {
        #[cfg(test)]
        path,
        durability,
        finish: false,
        check: Some(check),
    })
    .write_in(path, preparation_directory, bytes)
    .map_err(publication)
}

fn replace(
    path: &Path,
    bytes: &[u8],
    durability: &dyn Durability,
    finish: bool,
    check: Option<&dyn Fn() -> Result<()>>,
    prefix: &'static str,
) -> Result<()> {
    check_destination(path, durability)?;
    atomic_fs::Publisher::new((hive_setup_support::names::PUBLICATION_OPTIONS).with_prefix(prefix))
        .with_barriers(&Publication {
            #[cfg(test)]
            path,
            durability,
            finish,
            check,
        })
        .write(path, bytes)
        .map_err(publication)
}

fn check_destination(path: &Path, durability: &dyn Durability) -> Result<()> {
    let parent = path.parent().ok_or_else(Error::parent_directory_missing)?;
    dir(parent, false, durability)?;
    match fs::symlink_metadata(path) {
        Ok(meta) => check_file(&meta),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

struct Publication<'a> {
    #[cfg(test)]
    path: &'a Path,
    durability: &'a dyn Durability,
    finish: bool,
    check: Option<&'a dyn Fn() -> Result<()>>,
}

pub(super) fn publication(cause: atomic_fs::Error) -> Error {
    // A nested record can commit inside an unpublished initialization directory.
    // This boundary reports publication of the enclosing object only.
    let published = cause.published();
    let mut error = crate::error::publication(cause);
    error.file_committed = published;
    error
}
impl atomic_fs::Barriers for Publication<'_> {
    fn sync_file(&self, file: &File) -> io::Result<()> {
        order_sync(file, self.durability)
    }
    fn sync_directory(&self, file: &File) -> io::Result<()> {
        self.durability.fsync(file)
    }
    fn commit(&self, file: &File) -> io::Result<()> {
        if self.finish {
            self.durability.full_flush(file)
        } else {
            Ok(())
        }
    }
    fn observe(&self, step: atomic_fs::Step) -> io::Result<()> {
        #[cfg(test)]
        checkpoint(self.path, step)?;
        if step == atomic_fs::Step::BeforePublish
            && let Some(check) = self.check
        {
            check().map_err(io::Error::other)?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(super) fn checkpoint(
    path: &Path,
    step: impl Into<hive_setup_support::checkpoints::Checkpoint>,
) -> io::Result<()> {
    hooks::step(path, step)
}
#[cfg(test)]
pub(crate) mod hooks;

fn order_sync(file: &File, durability: &dyn Durability) -> io::Result<()> {
    durability.fsync(file)?;
    #[cfg(target_os = "macos")]
    barrier_after_sync(file, durability)?;
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
fn barrier_after_sync(file: &File, durability: &dyn Durability) -> io::Result<()> {
    match durability.ordering_barrier(file) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(libc::EINVAL | libc::ENOTSUP | libc::ENOSYS)
            ) =>
        {
            durability.full_flush(file)?;
            Ok(())
        }
        Err(error) => Err(error),
    }
}
