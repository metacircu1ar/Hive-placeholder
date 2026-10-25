//! Keep publication evidence and owned preparation directories on ambiguous cleanup.
use crate::error::{Error, Result};
use std::{error::Error as _, path::Path};
use tempfile::TempDir;
mod directory;

pub(crate) fn error(cause: atomic_fs::Error) -> Error {
    let public = cause
        .io_error()
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<Error>())
        .cloned()
        .unwrap_or_else(|| Error::state_io("Cannot safely access Hive state."));
    public.with_source(cause)
}

pub(crate) fn prepare<T>(
    directory: TempDir,
    build: impl FnOnce(&Path) -> Result<T>,
) -> Result<(TempDir, T)> {
    let path = directory.path().to_owned();
    let mut owned = directory::Owned::new(directory)?;
    match build(&path) {
        Ok(value) => Ok((owned.release()?, value)),
        Err(error) => {
            let retained = error
                .source()
                .and_then(|cause| cause.downcast_ref::<atomic_fs::Error>())
                .is_some_and(|cause| cause.retained().is_some());
            if retained {
                // A failed identity check must not be undone by recursive TempDir drop.
                owned.retain();
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests;
