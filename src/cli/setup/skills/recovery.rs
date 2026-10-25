//! Discharge previous intent obligations before replacing their ownership record.
use super::{Result, invalid, owned::digest, publication, read_file, read_owned, safe};
use atomic_fs::{Barriers, DefaultBarriers, Step};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

pub(super) fn resync(path: &Path, expected: Option<&str>) -> Result<()> {
    synchronize(path, expected, None)
        .map_err(|source| publication::error(atomic_fs::Error::new(source, true)))
}

pub(super) fn synchronize(
    path: &Path,
    expected: Option<&str>,
    held_directory: Option<File>,
) -> io::Result<()> {
    safe(path).map_err(io::Error::other)?;
    let present = read_file(path).map_err(io::Error::other)?;
    if present.as_ref().map(|(_, bytes)| digest(bytes)).as_deref() != expected {
        return Err(changed());
    }
    let parent = existing_parent(path)?;
    let directory = match held_directory {
        Some(directory) => directory,
        None => OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_NONBLOCK)
            .open(&parent)?,
    };
    let check = || {
        safe(path).map_err(io::Error::other)?;
        if read_owned(path)
            .map_err(io::Error::other)?
            .as_deref()
            .map(digest)
            .as_deref()
            != expected
            || existing_parent(path)? != parent
        {
            return Err(changed());
        }
        if let Some((file, _)) = &present {
            same(file, path)?;
        }
        same(&directory, &parent)
    };
    check()?;
    if let Some((file, _)) = &present {
        publication::checkpoint(path, Step::BeforeFileSync)?;
        check()?;
        DefaultBarriers.sync_file(file)?;
        publication::checkpoint(path, Step::AfterFileSync)?;
        check()?;
    }
    publication::checkpoint(path, Step::BeforeDirectorySync)?;
    check()?;
    DefaultBarriers.sync_directory(&directory)?;
    publication::checkpoint(path, Step::AfterDirectorySync)?;
    check()
}

fn existing_parent(path: &Path) -> io::Result<PathBuf> {
    for parent in path.ancestors().skip(1) {
        match fs::symlink_metadata(parent) {
            Ok(m)
                if m.is_dir()
                    && hive_setup_support::directories::owned(
                        m.uid(),
                        m.mode(),
                        rustix::process::getuid().as_raw(),
                    ) =>
            {
                return Ok(parent.to_owned());
            }
            Ok(_) => return Err(changed()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
    }
    Err(changed())
}

pub(super) fn same(file: &File, path: &Path) -> io::Result<()> {
    let held = file.metadata()?;
    let named = fs::symlink_metadata(path)?;
    if named.file_type().is_symlink()
        || (held.dev(), held.ino()) != (named.dev(), named.ino())
        || held.uid() != rustix::process::getuid().as_raw()
        || if held.is_dir() {
            !hive_setup_support::directories::owned(
                held.uid(),
                held.mode(),
                rustix::process::geteuid().as_raw(),
            )
        } else {
            held.mode() & 0o022 != 0
        }
        || (held.is_file() && held.nlink() != 1)
    {
        return Err(changed());
    }
    Ok(())
}
fn changed() -> io::Error {
    io::Error::other(invalid(
        "Skill changed during recovery; refusing to finalize ownership.",
    ))
}
