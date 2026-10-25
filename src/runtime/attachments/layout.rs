//! Descriptor-bound private snapshot directories; callers retain admission ownership.
use crate::{
    error::{Error, Result},
    os,
};
use atomic_fs::Step;
use rustix::fs::{Mode, OFlags, open, openat};
use std::{
    fs::{self, File, Metadata},
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub(super) fn invalid() -> Error {
    Error::unsafe_state("Attachment directory contents or ownership changed.")
}
pub(super) fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(Error::state_io_from(error)),
    }
}
fn private(metadata: &Metadata, directory: bool) -> Result<()> {
    if metadata.uid() != os::effective_uid()
        || metadata.mode() & os::PRIVATE_ACCESS_MASK != 0
        || if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file() || metadata.nlink() != 1
        }
    {
        return Err(invalid());
    }
    Ok(())
}
fn same(a: &File, b: &File) -> Result<bool> {
    let a = a.metadata().map_err(Error::state_io_from)?;
    let b = b.metadata().map_err(Error::state_io_from)?;
    Ok(a.dev() == b.dev() && a.ino() == b.ino())
}
pub(super) fn boundary(path: &Path, step: Step) -> Result<()> {
    #[cfg(test)]
    let result = crate::state::sync_tests::step(path, step);
    #[cfg(not(test))]
    let result = {
        let _ = path;
        atomic_fs::Barriers::observe(&atomic_fs::DefaultBarriers, step)
    };
    result.map_err(Error::state_io_from)
}

pub(super) struct Directory {
    pub path: PathBuf,
    pub file: File,
}
impl Directory {
    pub(super) fn open(path: &Path) -> Result<Self> {
        crate::state::check_path(path, true)?;
        let file = File::from(
            open(path, directory_flags(), Mode::empty())
                .map_err(|e| Error::state_io_from(e.into()))?,
        );
        let bound = Self {
            path: path.into(),
            file,
        };
        bound.check()?;
        Ok(bound)
    }
    pub(super) fn child(&self, key: &str) -> Result<Option<Self>> {
        self.check()?;
        let path = self.path.join(key);
        if !exists(&path)? {
            return Ok(None);
        }
        crate::state::check_path(&path, true)?;
        let file = File::from(
            openat(&self.file, key, directory_flags(), Mode::empty())
                .map_err(|e| Error::state_io_from(e.into()))?,
        );
        let child = Self { path, file };
        self.check()?;
        child.check()?;
        Ok(Some(child))
    }
    pub(super) fn check(&self) -> Result<()> {
        let named = fs::symlink_metadata(&self.path).map_err(Error::state_io_from)?;
        let held = self.file.metadata().map_err(Error::state_io_from)?;
        private(&named, true)?;
        if named.dev() != held.dev() || named.ino() != held.ino() {
            return Err(invalid());
        }
        Ok(())
    }
    pub(super) fn sync(&self) -> Result<()> {
        boundary(&self.path, Step::BeforeDirectorySync)?;
        self.check()?;
        self.file.sync_all().map_err(Error::state_io_from)?;
        boundary(&self.path, Step::AfterDirectorySync)?;
        self.check()
    }
    /// Bounded preflight: inspecting a second entry is enough to refuse mutation.
    pub(super) fn entry(&self, expected: Option<&str>) -> Result<Option<Payload>> {
        self.check()?;
        let mut entries = fs::read_dir(&self.path).map_err(Error::state_io_from)?;
        let Some(entry) = entries.next() else {
            self.check()?;
            return Ok(None);
        };
        let entry = entry.map_err(Error::state_io_from)?;
        if entries
            .next()
            .transpose()
            .map_err(Error::state_io_from)?
            .is_some()
        {
            return Err(invalid());
        }
        let name = entry.file_name().into_string().map_err(|_| invalid())?;
        if !super::valid_name(&name) || expected.is_some_and(|expected| name != expected) {
            return Err(invalid());
        }
        crate::state::check_path(&entry.path(), false)?;
        let file = File::from(
            openat(
                &self.file,
                &name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(|e| Error::state_io_from(e.into()))?,
        );
        private(&file.metadata().map_err(Error::state_io_from)?, false)?;
        self.check()?;
        Ok(Some(Payload { name, file }))
    }
    pub(super) fn check_entry(&self, expected: Option<&Payload>) -> Result<()> {
        let actual = self.entry(expected.map(|entry| entry.name.as_str()))?;
        match (expected, actual) {
            (None, None) => Ok(()),
            (Some(expected), Some(actual)) if same(&expected.file, &actual.file)? => Ok(()),
            _ => Err(invalid()),
        }
    }
}
fn directory_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}
pub(super) struct Payload {
    pub name: String,
    pub file: File,
}
