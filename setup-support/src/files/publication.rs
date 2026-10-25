//! Private bounded records with retained publication and native error evidence.
use super::{File, MAX_RECORD, Path, Result, directory, fs, invalid, record, record_bytes};
use atomic_fs::{Barriers, PreparedCreate, Step};
use std::{io, os::unix::fs::MetadataExt};

pub fn write(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    if bytes.len() as u64 > MAX_RECORD {
        return Err(invalid("Setup record exceeds 256 KiB."));
    }
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Record needs a parent."))?;
    if let Some(file) = record(path)?
        && record_bytes(&file)? == bytes
    {
        return resync(path, &file)
            .map_err(|source| publication(atomic_fs::Error::new(source, true)));
    }
    directory(parent)?;
    let barriers = Publication(path);
    if replace {
        atomic_fs::Publisher::new(crate::names::PUBLICATION_OPTIONS)
            .with_barriers(&barriers)
            .write(path, bytes)
    } else {
        PreparedCreate::new(path, crate::names::PUBLICATION_OPTIONS)
            .map_err(publication)?
            .publish_with(bytes, &barriers)
    }
    .map_err(publication)
}

fn publication(cause: atomic_fs::Error) -> io::Error {
    io::Error::new(cause.io_error().kind(), cause)
}

fn resync(path: &Path, file: &File) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Record needs a parent."))?;
    directory(parent)?;
    let held_parent = File::open(parent)?;
    let barriers = Publication(path);
    same_name(file, path)?;
    same_name(&held_parent, parent)?;
    barriers.observe(Step::BeforeFileSync)?;
    barriers.sync_file(file)?;
    barriers.observe(Step::AfterFileSync)?;
    same_name(file, path)?;
    barriers.observe(Step::BeforeDirectorySync)?;
    barriers.sync_directory(&held_parent)?;
    barriers.observe(Step::AfterDirectorySync)?;
    same_name(file, path)?;
    same_name(&held_parent, parent)
}

fn same_name(file: &File, path: &Path) -> Result<()> {
    let held = file.metadata()?;
    let named = fs::symlink_metadata(path)?;
    if named.file_type().is_symlink()
        || (held.dev(), held.ino()) != (named.dev(), named.ino())
        || held.uid() != rustix::process::getuid().as_raw()
        || (held.is_file() && (held.nlink() != 1 || held.mode() & 0o077 != 0))
    {
        return Err(invalid("Record publication identity changed."));
    }
    Ok(())
}

struct Publication<'a>(&'a Path);
impl Barriers for Publication<'_> {
    fn observe(&self, step: Step) -> Result<()> {
        #[cfg(not(test))]
        let _ = (self.0, step);
        #[cfg(test)]
        tests::checkpoint(self.0, step)?;
        #[cfg(test)]
        if step == Step::BeforeDirectorySync {
            super::before_parent_sync(self.0)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
