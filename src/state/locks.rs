//! flock ownership follows the file description, including inherited copies.
use super::private_file;
use crate::{
    error::{Error, Result},
    os,
};
use std::{
    fs::File,
    io::ErrorKind,
    os::fd::{AsFd, BorrowedFd},
    path::Path,
};
mod durable;

/// Closing the description releases flock. Never call unlock: inherited child fds
/// must keep ownership after the parent closes its copy or is killed.
pub struct Lock(File, Option<super::admission::Shared>);
impl Lock {
    pub(crate) fn with_admission(mut self, admission: super::admission::Shared) -> Self {
        self.1 = Some(admission);
        self
    }
    pub(crate) fn admission(&self) -> Option<super::admission::Shared> {
        self.1.clone()
    }
    pub fn shared(path: &Path) -> Result<Self> {
        let file = private_file(path, true)?;
        if let Err(cause) = os::shared_lock(&file, true) {
            return Err(
                Error::profile_busy("An exclusive operation owns this profile.").with_source(cause),
            );
        }
        Ok(Self(file, None))
    }
    /// The index lock is held only for metadata transactions, never a provider.
    pub fn index(path: &Path) -> Result<Self> {
        let file = private_file(path, true)?;
        #[cfg(test)]
        super::sync_tests::step(path, atomic_fs::Step::BeforePublish)
            .map_err(Error::state_io_from)?;
        loop {
            match os::exclusive_lock(&file, false) {
                Ok(()) => return Ok(Self(file, None)),
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(Error::state_io_from(error)),
            }
        }
    }
    pub fn acquire(path: &Path) -> Result<Self> {
        Self::exclusive(path, true)
    }
    /// GC must never recreate a vanished ownership marker or lock a replacement.
    pub(crate) fn acquire_existing(path: &Path) -> Result<Self> {
        Self::exclusive(path, false)
    }
    fn exclusive(path: &Path, create: bool) -> Result<Self> {
        let file = private_file(path, create)?;
        if let Err(e) = os::exclusive_lock(&file, true) {
            return Err(if e.kind() == ErrorKind::WouldBlock {
                Error::profile_busy(
                    "Another auth operation owns this profile. Retry after it finishes.",
                )
                .with_source(e)
            } else {
                Error::state_io_from(e)
            });
        }
        if super::LockIdentity::from_fd(&file)? != super::LockIdentity::at(path)? {
            return Err(super::unsafe_state());
        }
        Ok(Self(file, None))
    }
    /// A shared hold on an existing lock file: never creates one, and refuses a
    /// replaced file. Busy while an exclusive owner holds it.
    pub(crate) fn shared_existing(path: &Path) -> Result<Self> {
        let file = private_file(path, false)?;
        if let Err(e) = os::shared_lock(&file, true) {
            return Err(if e.kind() == ErrorKind::WouldBlock {
                Error::profile_busy("An exclusive owner holds this lock.").with_source(e)
            } else {
                Error::state_io_from(e)
            });
        }
        if super::LockIdentity::from_fd(&file)? != super::LockIdentity::at(path)? {
            return Err(super::unsafe_state());
        }
        Ok(Self(file, None))
    }
    /// Turn this exclusive hold into a shared one. flock converts by releasing
    /// first, so callers must serialize every other request on this file.
    pub(crate) fn share(&self) -> Result<()> {
        os::shared_lock(&self.0, true).map_err(Error::state_io_from)
    }
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}
