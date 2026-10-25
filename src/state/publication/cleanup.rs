//! Nonrecursive preparation cleanup while the actual publishers are excluded.
use crate::{
    error::{Error, Result},
    state::{self, Lock, LockIdentity, State},
};
use atomic_fs::{Barriers, Step};
use rustix::fs::{AtFlags, Mode, OFlags, open, openat, unlinkat};
use std::{
    ffi::OsStr,
    fs::{self, File},
    io,
    os::unix::fs::MetadataExt,
    path::Path,
};

fn invalid() -> Error {
    Error::unsafe_state("State preparation ownership changed or contains an unsafe entry.")
}
pub(in crate::state) fn ownership(lock: &Lock, path: &Path) -> Result<()> {
    if LockIdentity::from_fd(lock.fd())? != LockIdentity::at(path)? {
        return Err(invalid());
    }
    Ok(())
}

impl State {
    /// Caller holds checked daemon ownership; never call from a read-only command.
    pub(crate) fn cleanup_preparations(&self, daemon_lock: &Lock) -> Result<()> {
        let _admission = self.admission()?;
        let runtime = self.root.join("runtime");
        let daemon_path = runtime.join("daemon.lock");
        let check_daemon = || ownership(daemon_lock, &daemon_path);
        check_daemon()?;
        self.check_format()?;
        crate::runtime::check_existing_format(&runtime.join("runtime.sqlite"))?;
        let output = runtime.join("output");
        let output_exists = match fs::symlink_metadata(&output) {
            Ok(_) => {
                state::check_path(&output, true)?;
                true
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(Error::state_io_from(error)),
        };
        {
            let index = self.metadata_lock()?;
            self.read_index()?;
            sweep(&self.root, || {
                check_daemon()?;
                ownership(&index, &self.root.join("index.lock"))
            })?;
        }
        sweep(&runtime, check_daemon)?;
        if output_exists {
            sweep(&output, check_daemon)?;
        }
        Ok(())
    }
}

fn boundary(path: &Path, step: Step) -> Result<()> {
    #[cfg(test)]
    let result = state::sync_tests::step(path, step);
    #[cfg(not(test))]
    let result = {
        let _ = path;
        atomic_fs::DefaultBarriers.observe(step)
    };
    result.map_err(Error::state_io_from)
}

pub(in crate::state) fn sweep(path: &Path, ownership: impl Fn() -> Result<()>) -> Result<()> {
    ownership()?;
    let directory = Directory::open(path)?;
    for entry in fs::read_dir(path).map_err(Error::state_io_from)? {
        let entry = entry.map_err(Error::state_io_from)?;
        let name = entry.file_name();
        if !hive_setup_support::names::state_preparation_name(&name) {
            continue;
        }
        let file = directory.entry(&name)?;
        let target = path.join(&name);
        boundary(&target, Step::BeforeRemove)?;
        ownership()?;
        directory.check()?;
        let named = directory.entry(&name)?;
        if !same(&file, &named)? {
            return Err(invalid());
        }
        unlinkat(&directory.file, &name, AtFlags::empty())
            .map_err(|cause| Error::state_io_from(cause.into()))?;
        boundary(&target, Step::AfterRemove)?;
    }
    // Repeat on an empty retry: a previous unlink may have failed its parent sync.
    boundary(path, Step::BeforeDirectorySync)?;
    ownership()?;
    directory.check()?;
    atomic_fs::DefaultBarriers
        .sync_directory(&directory.file)
        .map_err(Error::state_io_from)?;
    boundary(path, Step::AfterDirectorySync)?;
    ownership()?;
    directory.check()
}

struct Directory<'a> {
    path: &'a Path,
    file: File,
}
impl<'a> Directory<'a> {
    fn open(path: &'a Path) -> Result<Self> {
        state::check_path(path, true)?;
        let file = File::from(
            open(
                path,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|cause| Error::state_io_from(cause.into()))?,
        );
        let directory = Self { path, file };
        directory.check()?;
        Ok(directory)
    }
    fn check(&self) -> Result<()> {
        state::check_path(self.path, true)?;
        let named = fs::symlink_metadata(self.path).map_err(Error::state_io_from)?;
        let held = self.file.metadata().map_err(Error::state_io_from)?;
        if named.dev() != held.dev() || named.ino() != held.ino() {
            return Err(invalid());
        }
        Ok(())
    }
    fn entry(&self, name: &OsStr) -> Result<File> {
        self.check()?;
        state::check_path(&self.path.join(name), false)?;
        let file = File::from(
            openat(
                &self.file,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(|cause| Error::state_io_from(cause.into()))?,
        );
        state::checked_file(file)
    }
}
fn same(a: &File, b: &File) -> Result<bool> {
    let a = a.metadata().map_err(Error::state_io_from)?;
    let b = b.metadata().map_err(Error::state_io_from)?;
    Ok(a.dev() == b.dev() && a.ino() == b.ino())
}

#[cfg(test)]
mod tests;
