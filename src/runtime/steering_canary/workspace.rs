//! Disposable private input; capability proof and native ownership stay separate.
use crate::{
    error::{Error, Result},
    os,
};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

pub(super) fn create(parent: &Path) -> Result<tempfile::TempDir> {
    let directory = tempfile::Builder::new()
        .prefix("steering-canary-")
        .permissions(fs::Permissions::from_mode(os::PRIVATE_DIRECTORY_MODE))
        .tempdir_in(parent)
        .map_err(Error::state_io_from)?;
    crate::publication::prepare(directory, |path| {
        atomic_fs::PreparedCreate::new(
            path.join("fixed.txt"),
            (hive_setup_support::names::PUBLICATION_OPTIONS)
                .with_durability(atomic_fs::Durability::AtomicVisible),
        )
        .and_then(|prepared| prepared.publish(b"Hive native steering boundary.\n"))
        .map_err(crate::publication::error)
    })
    .map(|(directory, ())| directory)
}

#[cfg(test)]
mod tests;
