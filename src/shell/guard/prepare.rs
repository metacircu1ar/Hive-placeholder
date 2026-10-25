//! Publish one complete shell-control tree while preserving filesystem cloning.
use crate::{error::Result, os};
use std::{fs, io::Write, os::unix::fs::PermissionsExt, path::Path};

pub(super) fn publish(
    path: &Path,
    binary: &Path,
    instructions: &str,
    barriers: &impl atomic_fs::Barriers,
) -> Result<()> {
    atomic_fs::Publisher::new(
        (hive_setup_support::names::PUBLICATION_OPTIONS)
            .with_durability(atomic_fs::Durability::AtomicVisible),
    )
    .with_barriers(barriers)
    .staged_dir(path, |stage| {
        // Source modes are confined to the unpublished 0700 directory. Set the
        // final modes before publishing it; copying to a new name retains cloning.
        for name in ["hive-bash", "hive-shell"] {
            let path = stage.join(name);
            fs::copy(binary, &path)?;
            fs::set_permissions(path, fs::Permissions::from_mode(os::READ_EXECUTE_MODE))?;
        }
        os::private_file(&stage.join("instructions"), true)?.write_all(instructions.as_bytes())
    })
    .map_err(crate::publication::error)
}
