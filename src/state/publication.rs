//! Buffer JSON once while preserving each caller's existing durability callbacks.
use super::{check_path, corrupt, unsafe_state};
use crate::error::{Error, Result};
use atomic_fs::Barriers;
use serde::Serialize;
use std::{
    fs::{self, File},
    io,
    path::Path,
};
pub(super) mod cleanup;

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    write_json_with_sync(path, value, |file, directory| {
        crate::observability::state_sync::barrier(file, "state", directory)
    })
}

pub(crate) fn write_json_with_sync(
    path: &Path,
    value: &impl Serialize,
    sync: impl Fn(&File, bool) -> io::Result<()>,
) -> Result<()> {
    check_path(path.parent().ok_or_else(unsafe_state)?, true)?;
    match fs::symlink_metadata(path) {
        Ok(_) => check_path(path, false)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(Error::state_io_from(error)),
    }
    let mut bytes = serde_json::to_vec(value).map_err(|cause| corrupt().with_source(cause))?;
    bytes.push(b'\n');
    atomic_fs::Publisher::new(
        (hive_setup_support::names::PUBLICATION_OPTIONS)
            .with_prefix(hive_setup_support::names::STATE_PREFIX),
    )
    .with_barriers(&Sync {
        sync: &sync,
        #[cfg(test)]
        path,
    })
    .write(path, &bytes)
    .map_err(Error::state_publication)
}

struct Sync<'a, F> {
    sync: &'a F,
    #[cfg(test)]
    path: &'a Path,
}
impl<F: Fn(&File, bool) -> io::Result<()>> Barriers for Sync<'_, F> {
    fn sync_file(&self, file: &File) -> io::Result<()> {
        (self.sync)(file, false)
    }
    fn sync_directory(&self, file: &File) -> io::Result<()> {
        (self.sync)(file, true)
    }
    #[cfg(test)]
    fn observe(&self, step: atomic_fs::Step) -> io::Result<()> {
        super::sync_tests::step(self.path, step)
    }
}

/// Recognize only current helper preparations; recognition grants no deletion authority.
pub(crate) fn staging_file(path: &Path) -> Result<bool> {
    let staging = path
        .file_name()
        .is_some_and(hive_setup_support::names::state_preparation_name)
        || path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| {
                name.strip_prefix(".hive-tmp-").is_some_and(|suffix| {
                    (suffix.len() == 6 && suffix.bytes().all(|b| b.is_ascii_alphanumeric()))
                        || (suffix.len() == 16
                            && suffix
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
                })
            });
    if staging {
        check_path(path, false)?;
    }
    Ok(staging)
}

#[cfg(test)]
mod tests;
