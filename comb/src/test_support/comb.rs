use crate::{Comb, Result};
use tempfile::TempDir;

pub(crate) struct TestComb {
    // Fields drop in declaration order: release the writer before cleaning its directory.
    pub(crate) comb: Comb,
    pub(crate) root: TempDir,
}
impl TestComb {
    pub(crate) fn new() -> Result<Self> {
        let root = tempfile::tempdir()?;
        let comb = Comb::init(root.path())?;
        Ok(Self { comb, root })
    }
    pub(crate) fn short() -> Result<Self> {
        let root = private_directory()?;
        let comb = Comb::init(root.path())?;
        Ok(Self { comb, root })
    }
    pub(crate) fn into_parts(self) -> (TempDir, Comb) {
        (self.root, self.comb)
    }
}

pub(crate) fn private_directory() -> std::io::Result<TempDir> {
    use std::os::unix::fs::PermissionsExt;
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
}
