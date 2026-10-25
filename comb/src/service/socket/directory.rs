//! Private socket parents with a checked route from the filesystem root.
mod access;
use super::{Error, Result, files::native};
use access::Directory;
use rustix::fs::{self, Mode, Stat};
use std::{
    fs::File,
    path::{Component, Path},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Existing,
    AllowMissing,
    Create,
}

pub(crate) fn check(path: &Path, action: Action) -> Result<Option<File>> {
    let names = names(path)?;
    let mut held = Directory::root()?;
    let mut parent = None;
    ancestor(&fs::fstat(&held.file).map_err(native)?)?;
    for name in names {
        let next = match held.child(name) {
            Ok(next) => next,
            Err(rustix::io::Errno::NOENT) if action == Action::AllowMissing => return Ok(None),
            Err(rustix::io::Errno::NOENT) if action == Action::Create => {
                // A retry can find the last created directory without its name
                // being durable yet. Flush that prefix before extending it.
                access::barrier(&held, parent.as_ref())?;
                create(&held, name)?
            }
            Err(error) => return Err(native(error)),
        };
        ancestor(&fs::fstat(&next.file).map_err(native)?)?;
        parent = Some(held);
        held = next;
    }
    private(&fs::fstat(&held.file).map_err(native)?)?;
    if action == Action::Create {
        access::barrier(&held, parent.as_ref())?;
    }
    Ok(Some(held.readable()?))
}

fn names(path: &Path) -> Result<Vec<&std::ffi::OsStr>> {
    if !path.is_absolute() {
        return Err(Error::unsafe_socket_parent());
    }
    path.components()
        .filter(|c| *c != Component::RootDir)
        .map(|c| match c {
            Component::Normal(name) => Ok(name),
            _ => Err(Error::unsafe_socket_parent()),
        })
        .collect()
}
fn ancestor(stat: &Stat) -> Result<()> {
    if stat.st_mode & libc::S_IFMT != libc::S_IFDIR
        || !hive_setup_support::directories::ancestor(
            stat.st_uid,
            stat.st_mode,
            rustix::process::geteuid().as_raw(),
        )
    {
        return Err(Error::unsafe_socket_parent());
    }
    Ok(())
}
fn private(stat: &Stat) -> Result<()> {
    if stat.st_uid != rustix::process::geteuid().as_raw() || stat.st_mode & 0o077 != 0 {
        return Err(Error::unsafe_socket_parent());
    }
    Ok(())
}
fn create(parent: &Directory, name: &std::ffi::OsStr) -> Result<Directory> {
    match fs::mkdirat(&parent.file, name, Mode::RWXU) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
        Err(error) => return Err(native(error)),
    }
    let directory = parent.child(name).map_err(native)?;
    private(&fs::fstat(&directory.file).map_err(native)?)?;
    access::barrier(&directory, Some(parent))?;
    Ok(directory)
}
