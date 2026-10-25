//! Cold-path durability before a new credential lock identity enters the index.
use super::{Error, Lock, Result};
use crate::state::{LockIdentity, unsafe_state};
use atomic_fs::{Barriers, Step};
use std::{fs::File, io, path::Path};

struct Creation;
impl Barriers for Creation {
    #[cfg(test)]
    fn observe(&self, step: Step) -> io::Result<()> {
        tests::step(step)
    }
}

impl Lock {
    /// Never replace the inode: ownership follows this held file description.
    pub(in crate::state) fn acquire_durable(path: &Path) -> Result<Self> {
        let lock = Self::acquire(path)?;
        lock.sync_creation(path, &Creation)?;
        // A replaced marker during the barriers cannot be committed as our lock.
        if LockIdentity::from_fd(lock.fd())? != LockIdentity::at(path)? {
            return Err(unsafe_state());
        }
        Ok(lock)
    }

    pub(crate) fn sync_creation(&self, path: &Path, barriers: &impl Barriers) -> Result<()> {
        let parent = path.parent().ok_or_else(unsafe_state)?;
        let sync = || -> io::Result<()> {
            barriers.observe(Step::BeforeFileSync)?;
            barriers.sync_file(&self.0)?;
            barriers.observe(Step::AfterFileSync)?;
            barriers.observe(Step::BeforeDirectorySync)?;
            barriers.sync_directory(&File::open(parent)?)?;
            barriers.observe(Step::AfterDirectorySync)
        };
        // Retain the file for retry even when the final barrier failed. Dropping
        // only our descriptor preserves inherited flock ownership semantics.
        sync().map_err(Error::state_io_from)
    }
}

#[cfg(test)]
mod tests;
