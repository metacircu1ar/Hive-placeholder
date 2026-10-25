//! Descriptor-relative socket namespace access and stable exclusive ownership.
use super::record::Node;
use crate::{Error, Result};
use rustix::fs::{self, AtFlags, Mode, OFlags, Stat};
use std::{
    fs::File,
    path::{Path, PathBuf},
};

pub(super) fn native(error: rustix::io::Errno) -> Error {
    std::io::Error::from(error).into()
}
pub(super) struct Parent {
    pub file: File,
    path: PathBuf,
    pub node: Node,
}
impl Parent {
    pub fn open(path: &Path) -> Result<Self> {
        let file = super::directory::check(path, super::directory::Action::Existing)?
            .ok_or_else(Error::unsafe_socket_parent)?;
        let node = Node::of(&fs::fstat(&file).map_err(native)?);
        let parent = Self {
            file,
            path: path.into(),
            node,
        };
        parent.check()?;
        Ok(parent)
    }
    pub fn check(&self) -> Result<()> {
        let current = super::directory::check(&self.path, super::directory::Action::Existing)?
            .ok_or_else(Error::unsafe_socket_parent)?;
        if Node::of(&fs::fstat(&current).map_err(native)?) != self.node {
            return Err(Error::unsafe_directory());
        }
        Ok(())
    }
    pub fn stat(&self, name: &std::ffi::OsStr) -> Result<Option<Stat>> {
        match fs::statat(&self.file, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => Ok(Some(stat)),
            Err(rustix::io::Errno::NOENT) => Ok(None),
            Err(error) => Err(native(error)),
        }
    }
    pub fn remove(&self, name: &std::ffi::OsStr) -> Result<()> {
        self.check()?;
        fs::unlinkat(&self.file, name, AtFlags::empty()).map_err(native)?;
        self.file.sync_all()?;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    pub fn private_socket(&self, name: &std::ffi::OsStr, expected: &Node) -> Result<()> {
        use std::os::fd::AsRawFd;
        // Linux fchmodat cannot implement no-follow with the pinned rustix.
        // O_PATH pins the actual object; /proc resolves that descriptor, never
        // a caller-controlled symlink. No less-safe pathname fallback.
        let fd = fs::openat(
            &self.file,
            name,
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(native)?;
        let stat = fs::fstat(&fd).map_err(native)?;
        if &Node::of(&stat) != expected || stat.st_mode & libc::S_IFMT != libc::S_IFSOCK {
            return Err(Error::unsafe_socket());
        }
        fs::chmod(
            format!("/proc/self/fd/{}", fd.as_raw_fd()),
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(native)
    }
    #[cfg(not(target_os = "linux"))]
    pub fn private_socket(&self, name: &std::ffi::OsStr, expected: &Node) -> Result<()> {
        if self.stat(name)?.as_ref().map(Node::of).as_ref() != Some(expected) {
            return Err(Error::unsafe_socket());
        }
        fs::chmodat(
            &self.file,
            name,
            Mode::RUSR | Mode::WUSR,
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(native)
    }
}
pub(super) struct Lock(File);
impl Lock {
    pub fn acquire(parent: &Parent, name: &str) -> Result<Self> {
        let file = File::from(
            fs::openat(
                &parent.file,
                name,
                OFlags::RDWR
                    | OFlags::CREATE
                    | OFlags::NOFOLLOW
                    | OFlags::NONBLOCK
                    | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(native)?,
        );
        private(&fs::fstat(&file).map_err(native)?)?;
        loop {
            match fs::flock(&file, fs::FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => break,
                Err(rustix::io::Errno::INTR) => {}
                Err(rustix::io::Errno::WOULDBLOCK) => return Err(Error::socket_in_use()),
                Err(error) => return Err(native(error)),
            }
        }
        let held = Self(file);
        parent.check()?;
        held.check(parent, name)?;
        parent.file.sync_all()?;
        Ok(held)
    }
    pub fn check(&self, parent: &Parent, name: &str) -> Result<()> {
        let expected = fs::fstat(&self.0).map_err(native)?;
        private(&expected)?;
        if parent.stat(name.as_ref())?.as_ref().map(Node::of) != Some(Node::of(&expected)) {
            return Err(Error::unsafe_lock());
        }
        Ok(())
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        while fs::flock(&self.0, fs::FlockOperation::Unlock) == Err(rustix::io::Errno::INTR) {}
    }
}
pub(super) fn private(stat: &Stat) -> Result<()> {
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG
        || stat.st_nlink != 1
        || stat.st_uid != rustix::process::geteuid().as_raw()
        || stat.st_mode & 0o077 != 0
    {
        return Err(Error::unsafe_file());
    }
    Ok(())
}
