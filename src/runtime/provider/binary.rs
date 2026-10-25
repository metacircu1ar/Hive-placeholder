//! Compare executable identity without retaining a native process or catalog.
use crate::error::{Error, Result};
use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BinaryIdentity {
    path: PathBuf,
    device: u64,
    inode: u64,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl BinaryIdentity {
    pub(crate) fn read(path: &Path) -> Result<Self> {
        let path = path.canonicalize().map_err(Error::state_io_from)?;
        let meta = path.metadata().map_err(Error::state_io_from)?;
        Ok(Self {
            path,
            device: meta.dev(),
            inode: meta.ino(),
            length: meta.len(),
            modified: (meta.mtime(), meta.mtime_nsec()),
            changed: (meta.ctime(), meta.ctime_nsec()),
        })
    }
    pub(crate) fn unchanged(&self) -> bool {
        Self::read(&self.path)
            .inspect_err(|error| {
                crate::observability::observation_failed("binary_identity_unchanged", error);
            })
            .is_ok_and(|current| current == *self)
    }
}
