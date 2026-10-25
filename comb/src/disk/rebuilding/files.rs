//! Descriptor-relative checks: the intent owns exact files, never a prefix.
use super::{Intent, NAMES};
use crate::{Error, Result, disk::Durability};
use rustix::fs::{AtFlags, FileType, Mode, OFlags, Stat};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    device: u64,
    inode: u64,
}
impl Identity {
    fn stat(meta: &Stat) -> Self {
        #[cfg(target_os = "macos")]
        let device = u64::from_ne_bytes(i64::from(meta.st_dev).to_ne_bytes());
        #[cfg(not(target_os = "macos"))]
        let device = meta.st_dev;
        Self {
            device,
            inode: meta.st_ino,
        }
    }
}

pub(super) struct Directory {
    path: PathBuf,
    file: File,
    identity: Identity,
}
impl Directory {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let file = File::from(
            rustix::fs::open(
                path,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(io::Error::from)?,
        );
        let meta = file.metadata()?;
        let value = Self {
            path: path.to_owned(),
            file,
            identity: Identity {
                device: meta.dev(),
                inode: meta.ino(),
            },
        };
        value.check()?;
        Ok(value)
    }
    pub(super) fn identity(&self) -> Identity {
        self.identity
    }
    fn check(&self) -> Result<()> {
        let meta = std::fs::symlink_metadata(&self.path)?;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || meta.uid() != rustix::process::geteuid().as_raw()
            || meta.mode() & 0o077 != 0
            || meta.dev() != self.identity.device
            || meta.ino() != self.identity.inode
        {
            return Err(Error::unsafe_index_removal());
        }
        Ok(())
    }
    pub(super) fn entry(&self, name: &str) -> Result<Option<Identity>> {
        self.check()?;
        let meta = match rustix::fs::statat(&self.file, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(meta) => meta,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(error) => return Err(io::Error::from(error).into()),
        };
        if FileType::from_raw_mode(meta.st_mode) != FileType::RegularFile
            || meta.st_nlink != 1
            || meta.st_uid != rustix::process::geteuid().as_raw()
            || meta.st_mode & 0o022 != 0
        {
            return Err(Error::unsafe_index_removal());
        }
        let identity = Identity::stat(&meta);
        if identity.device != self.identity.device {
            return Err(Error::unsafe_index_removal());
        }
        Ok(Some(identity))
    }
    pub(super) fn capture(&self) -> Result<[Option<Identity>; 8]> {
        let mut files = [None; 8];
        for (slot, name) in NAMES.iter().enumerate() {
            files[slot] = self.entry(name)?;
        }
        Ok(files)
    }
    pub(super) fn validate(&self, intent: &Intent) -> Result<()> {
        if self.identity != intent.parent {
            return Err(Error::unsafe_index_removal());
        }
        for (slot, name) in NAMES.iter().enumerate() {
            let actual = self.entry(name)?;
            let selected = slot < 4 || intent.reset_vectors;
            if actual != intent.files[slot] && !(selected && actual.is_none()) {
                return Err(Error::unsafe_index_removal());
            }
        }
        Ok(())
    }
    pub(super) fn remove(
        &self,
        name: &str,
        expected: Option<Identity>,
        durability: &dyn Durability,
    ) -> Result<()> {
        if let Some(actual) = self.entry(name)? {
            if Some(actual) != expected {
                return Err(Error::unsafe_index_removal());
            }
            #[cfg(test)]
            crate::disk::durable::checkpoint(&self.path.join(name), atomic_fs::Step::BeforeRemove)?;
            if self.entry(name)? != expected {
                return Err(Error::unsafe_index_removal());
            }
            rustix::fs::unlinkat(&self.file, name, AtFlags::empty()).map_err(io::Error::from)?;
            #[cfg(test)]
            crate::disk::durable::checkpoint(&self.path.join(name), atomic_fs::Step::AfterRemove)?;
        }
        self.sync(name, durability)?;
        if self.entry(name)?.is_some() {
            return Err(Error::unsafe_index_removal());
        }
        Ok(())
    }
    fn sync(&self, name: &str, durability: &dyn Durability) -> Result<()> {
        self.check()?;
        self.entry(name)?;
        #[cfg(test)]
        crate::disk::durable::checkpoint(
            &self.path.join(name),
            atomic_fs::Step::BeforeDirectorySync,
        )?;
        durability.fsync(&self.file)?;
        #[cfg(test)]
        crate::disk::durable::checkpoint(
            &self.path.join(name),
            atomic_fs::Step::AfterDirectorySync,
        )?;
        self.check()
    }
    pub(super) fn sync_record(&self, name: &str, durability: &dyn Durability) -> Result<File> {
        let expected = self.entry(name)?.ok_or_else(Error::unsafe_index_removal)?;
        let file = File::from(
            rustix::fs::openat(
                &self.file,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(io::Error::from)?,
        );
        crate::disk::check_file(&file.metadata()?)?;
        if Identity::stat(&rustix::fs::fstat(&file).map_err(io::Error::from)?) != expected {
            return Err(Error::unsafe_index_removal());
        }
        #[cfg(test)]
        crate::disk::durable::checkpoint(&self.path.join(name), atomic_fs::Step::BeforeFileSync)?;
        durability.fsync(&file)?;
        self.sync(name, durability)?;
        durability.full_flush(&file)?;
        #[cfg(test)]
        crate::disk::durable::checkpoint(&self.path.join(name), atomic_fs::Step::AfterFileSync)?;
        if self.entry(name)? != Some(expected) {
            return Err(Error::unsafe_index_removal());
        }
        Ok(file)
    }
    pub(super) fn finish_handoff(
        &self,
        population: &File,
        durability: &dyn Durability,
    ) -> Result<()> {
        #[cfg(test)]
        crate::disk::durable::checkpoint(
            &self.path.join(super::POPULATION),
            atomic_fs::Step::BeforeFileSync,
        )?;
        self.check()?;
        durability.full_flush(population)?;
        #[cfg(test)]
        crate::disk::durable::checkpoint(
            &self.path.join(super::POPULATION),
            atomic_fs::Step::AfterFileSync,
        )?;
        self.check()
    }
}
