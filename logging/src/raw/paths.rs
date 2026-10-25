use super::Owner;
use rustix::fs::{AtFlags, Mode, OFlags, open, openat};
use std::{
    fs::{File, Metadata},
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub(super) fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Raw log ownership or path changed",
    )
}
pub(super) fn matches(meta: &Metadata, stat: &rustix::fs::Stat) -> bool {
    // Darwin's dev_t is signed; std exposes its unsigned bit representation.
    #[cfg(target_os = "macos")]
    let device = u64::from(u32::from_ne_bytes(stat.st_dev.to_ne_bytes()));
    #[cfg(not(target_os = "macos"))]
    let device = stat.st_dev;
    meta.dev() == device && meta.ino() == stat.st_ino
}
fn private(meta: &Metadata, directory: bool) -> io::Result<()> {
    if meta.uid() != rustix::process::geteuid().as_raw()
        || meta.mode() & 0o077 != 0
        || if directory {
            !meta.is_dir()
        } else {
            !meta.is_file() || meta.nlink() != 1
        }
    {
        return Err(invalid());
    }
    Ok(())
}
pub(super) struct Directory {
    pub path: PathBuf,
    file: File,
}
impl Directory {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::from(open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?);
        private(&file.metadata()?, true)?;
        let result = Self {
            file,
            path: path.into(),
        };
        result.check()?;
        Ok(result)
    }
    pub fn check(&self) -> io::Result<()> {
        let current = File::from(open(
            &self.path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?);
        private(&current.metadata()?, true)?;
        if !matches(&self.file.metadata()?, &rustix::fs::fstat(&current)?) {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn file(&self, name: &str, flags: OFlags) -> io::Result<File> {
        let file = File::from(openat(
            &self.file,
            name,
            flags | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        private(&file.metadata()?, false)?;
        self.named(name, &file)?;
        Ok(file)
    }
    pub fn named(&self, name: &str, file: &File) -> io::Result<()> {
        let meta = file.metadata()?;
        private(&meta, false)?;
        if !matches(
            &meta,
            &rustix::fs::statat(&self.file, name, AtFlags::SYMLINK_NOFOLLOW)?,
        ) {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn optional(&self, name: &str) -> io::Result<()> {
        match self.file(name, OFlags::RDONLY) {
            Ok(_) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}
pub(super) struct Checked<'a, B> {
    pub owner: &'a Owner,
    pub barriers: &'a B,
    pub previous: &'a str,
}
impl<B: atomic_fs::Barriers> atomic_fs::Barriers for Checked<'_, B> {
    fn observe(&self, step: atomic_fs::Step) -> io::Result<()> {
        self.barriers.observe(step)?;
        self.owner.check()?;
        self.owner.directory.optional(self.previous)
    }
}
