//! Bounded reads and publication of a root's private selection records.
use super::{Result, invalid};
use serde::de::DeserializeOwned;
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

mod publication;
#[cfg(test)]
pub(crate) use publication::hooks;
pub(super) use publication::{Publish, write_json_hook};
pub(crate) use publication::{create_record, encode_record, sync_json, write_json};

pub(super) const RECORD_BYTES: u64 = 4 * 1024 * 1024;

pub(crate) fn owned(path: &Path, directory: bool) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    owned_metadata(path, &meta, directory)
}
fn owned_metadata(path: &Path, meta: &fs::Metadata, directory: bool) -> Result<()> {
    // SAFETY: geteuid takes no arguments and has no memory effects.
    let uid = unsafe { libc::geteuid() };
    if meta.file_type().is_symlink()
        || meta.is_dir() != directory
        || (!directory && !meta.is_file())
        || meta.uid() != uid
        || if directory {
            !hive_setup_support::directories::owned(meta.uid(), meta.mode(), uid)
        } else {
            meta.mode() & 0o022 != 0
        }
    {
        return Err(invalid(format!(
            "Refusing unowned, writable or linked managed path: {}",
            path.display()
        )));
    }
    Ok(())
}
pub(crate) fn directory(path: &Path) -> Result<()> {
    let absolute = std::path::absolute(path)?;
    let path = absolute.as_path();
    // Preserve provisioning's owned-path rejection code and context; actual
    // creation or durability failures below remain native I/O errors.
    match fs::symlink_metadata(path) {
        Ok(meta) => owned_metadata(path, &meta, true)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    hive_setup_support::files::directory(path).map_err(crate::Error::from)
}
pub(super) fn open(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)?;
    owned_metadata(path, &file.metadata()?, false)?;
    Ok(file)
}
pub(super) fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = open(path)?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).map_err(|_| invalid("Asset size overflow."))? > limit {
        return Err(invalid("Managed record exceeds its size bound."));
    }
    Ok(bytes)
}
pub(crate) fn json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    Ok(serde_json::from_slice(&read(path, RECORD_BYTES)?)?)
}
