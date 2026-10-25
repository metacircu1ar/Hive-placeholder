//! Inspect candidates with stat only: opening/closing a SQLite alias loses locks.
use super::unsafe_entry;
use crate::Result;
use rustix::fs::{AtFlags, FileType, Mode, OFlags, Stat};
use std::{ffi::OsStr, fs::File, io, os::unix::fs::MetadataExt, path::Path};

pub(super) struct Directory {
    pub file: File,
    pub identity: Stat,
    private: bool,
}

const FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

impl Directory {
    pub fn root(path: &Path, device: u64) -> Result<Self> {
        let file =
            File::from(rustix::fs::open(path, FLAGS, Mode::empty()).map_err(io::Error::from)?);
        if file.metadata()?.dev() != device {
            return Err(unsafe_entry());
        }
        let directory = Self::held(file, false)?;
        directory.check_root(path)?;
        Ok(directory)
    }
    fn held(file: File, private: bool) -> Result<Self> {
        let identity = rustix::fs::fstat(&file).map_err(io::Error::from)?;
        let directory = Self {
            file,
            identity,
            private,
        };
        directory.check(&identity)?;
        Ok(directory)
    }
    pub fn child(&self, name: &str, private: bool) -> Result<Self> {
        let file = File::from(
            rustix::fs::openat(&self.file, name, FLAGS, Mode::empty()).map_err(io::Error::from)?,
        );
        let directory = Self::held(file, private)?;
        directory.check_at(self, name)?;
        Ok(directory)
    }
    pub fn check_root(&self, path: &Path) -> Result<()> {
        self.check(&rustix::fs::lstat(path).map_err(io::Error::from)?)
    }
    pub fn check_at(&self, parent: &Self, name: &str) -> Result<()> {
        if self.identity.st_dev != parent.identity.st_dev {
            return Err(unsafe_entry());
        }
        self.check(
            &rustix::fs::statat(&parent.file, name, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(io::Error::from)?,
        )
    }
    fn check(&self, named: &Stat) -> Result<()> {
        let held = rustix::fs::fstat(&self.file).map_err(io::Error::from)?;
        for meta in [named, &held] {
            if FileType::from_raw_mode(meta.st_mode) != FileType::Directory
                || meta.st_uid != rustix::process::geteuid().as_raw()
                || if self.private {
                    meta.st_mode & 0o077 != 0
                } else {
                    !hive_setup_support::directories::owned(
                        meta.st_uid,
                        meta.st_mode,
                        rustix::process::geteuid().as_raw(),
                    )
                }
                || !same(meta, &self.identity)
            {
                return Err(unsafe_entry());
            }
        }
        Ok(())
    }
    pub fn entry(&self, name: &OsStr) -> Result<Stat> {
        let meta = rustix::fs::statat(&self.file, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(io::Error::from)?;
        safe_file(&meta, &self.identity)?;
        Ok(meta)
    }
}

pub(super) fn same(a: &Stat, b: &Stat) -> bool {
    a.st_dev == b.st_dev && a.st_ino == b.st_ino
}

pub(super) fn safe_file(meta: &Stat, parent: &Stat) -> Result<()> {
    if FileType::from_raw_mode(meta.st_mode) != FileType::RegularFile
        || meta.st_nlink != 1
        || meta.st_uid != rustix::process::geteuid().as_raw()
        || meta.st_mode & 0o077 != 0
        || meta.st_dev != parent.st_dev
    {
        return Err(unsafe_entry());
    }
    Ok(())
}
