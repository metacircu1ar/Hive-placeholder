//! Empty temporary wrappers are removed nonrecursively, including below public parents.
use super::removal::Identity;
use super::{Error, LockIdentity, Result, State, check_path, corrupt, exists, publication_lock};
use crate::state;
use atomic_fs::{Barriers, Step};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) struct MetadataRoot {
    path: PathBuf,
    identity: Identity,
}
pub(crate) struct Cleanup {
    pub removed: bool,
    pub warning: Option<Error>,
}
impl State {
    pub(crate) fn metadata_root_identity(&self) -> Result<MetadataRoot> {
        Ok(MetadataRoot {
            path: self.root.clone(),
            identity: Identity::at(&self.root)?,
        })
    }
    pub(crate) fn remove_empty_metadata_root(&self, expected: &MetadataRoot) -> Cleanup {
        let mut removed = false;
        let warning = self
            .remove_empty_metadata_root_checked(expected, &mut removed)
            .err();
        Cleanup { removed, warning }
    }
    fn remove_empty_metadata_root_checked(
        &self,
        expected: &MetadataRoot,
        removed: &mut bool,
    ) -> Result<()> {
        expected.check(&self.root)?;
        let _initialization = self.initialization_lock()?;
        let publication = publication_lock(self)?;
        expected.check(&self.root)?;
        for entry in fs::read_dir(&self.root).map_err(Error::state_io_from)? {
            let entry = entry.map_err(Error::state_io_from)?;
            match entry.file_name().to_str() {
                Some("native-probes.lock") => check_path(&entry.path(), false)?,
                Some("native-probes") => check_path(&entry.path(), true)?,
                Some("format") => {
                    crate::state::format::check(&self.root)?;
                }
                _ => return Err(corrupt()),
            }
        }
        let base = self.root.join("native-probes");
        if exists(&base)? {
            fs::remove_dir(&base).map_err(Error::state_io_from)?;
        }
        let lock_path = self.root.join("native-probes.lock");
        let boundary = state::journal::Boundary(&lock_path);
        boundary
            .observe(Step::BeforeRemove)
            .map_err(Error::state_io_from)?;
        expected.check(&self.root)?;
        if LockIdentity::from_fd(publication.fd())? != LockIdentity::at(&lock_path)? {
            return Err(corrupt());
        }
        fs::remove_file(&lock_path).map_err(Error::state_io_from)?;
        boundary
            .observe(Step::AfterRemove)
            .map_err(Error::state_io_from)?;
        expected.check(&self.root)?;
        // All normal publication entrants share initialization exclusion. An
        // unexpected entrant or replacement must retain the admission marker.
        for entry in fs::read_dir(&self.root).map_err(Error::state_io_from)? {
            if entry.map_err(Error::state_io_from)?.file_name() != "format" {
                return Err(corrupt());
            }
        }
        crate::state::format::remove(&self.root)?;
        // After marker removal only an empty, still-owned wrapper may remain.
        let boundary = state::journal::Boundary(&self.root);
        boundary
            .observe(Step::BeforeRemove)
            .map_err(Error::state_io_from)?;
        expected.check(&self.root)?;
        fs::remove_dir(&self.root).map_err(Error::state_io_from)?;
        *removed = true;
        boundary
            .observe(Step::AfterRemove)
            .map_err(Error::state_io_from)?;
        boundary
            .observe(Step::BeforeDirectorySync)
            .map_err(Error::state_io_from)?;
        // The user's destination parent need not be private.
        state::sync_directory(self.root.parent().ok_or_else(corrupt)?)
            .map_err(Error::state_io_from)?;
        boundary
            .observe(Step::AfterDirectorySync)
            .map_err(Error::state_io_from)
    }
}
impl MetadataRoot {
    fn check(&self, path: &Path) -> Result<()> {
        if path != self.path {
            return Err(corrupt());
        }
        self.identity.check(path)
    }
}
