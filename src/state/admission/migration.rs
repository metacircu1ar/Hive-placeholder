//! Exclusive migration authority also excludes native children which survived
//! the Rust owner. Keep the original credential descriptors for the whole upgrade.
use crate::{
    error::{Error, Result},
    state::{Lock, LockIdentity, State, check_path, corrupt, recovery, recovery_required},
};
use hive_setup_support::admission::Exclusive;
use std::os::unix::fs::MetadataExt;

pub struct MigrationOwnership {
    // Root admission must be the last owner released.
    native: Vec<(std::path::PathBuf, Lock)>,
    daemon: Option<(std::path::PathBuf, Lock)>,
    root: Exclusive,
    directory: (std::path::PathBuf, u64, u64),
}
impl MigrationOwnership {
    pub fn check(&self) -> Result<()> {
        self.root.check().map_err(Error::state_io_from)?;
        let (path, device, inode) = &self.directory;
        check_path(path, true)?;
        let metadata = std::fs::symlink_metadata(path).map_err(Error::state_io_from)?;
        if (metadata.dev(), metadata.ino()) != (*device, *inode) {
            return Err(corrupt());
        }
        for (path, lock) in self.native.iter().chain(self.daemon.iter()) {
            unchanged_marker(lock, path)?;
        }
        Ok(())
    }
}

impl State {
    /// No creation, repair or cleanup: uncertainty refuses the upgrade and
    /// preserves the existing native evidence. Safe for dry-run admission too.
    pub fn migration_ownership(&self) -> Result<MigrationOwnership> {
        let root = Exclusive::existing(&self.root, "hive-state").map_err(Error::state_io_from)?;
        if !super::super::format::marker(&self.root)? {
            return Err(corrupt());
        }
        let metadata = std::fs::symlink_metadata(&self.root).map_err(Error::state_io_from)?;
        let runtime = self.root.join("runtime");
        let daemon = if recovery::exists(&runtime)? {
            check_path(&runtime, true)?;
            let lock = Lock::acquire_existing(&runtime.join("daemon.lock"))?;
            crate::runtime::check_daemon_owner(&runtime.join("owner.json"), &lock)?;
            Some((runtime.join("daemon.lock"), lock))
        } else {
            None
        };
        let mut native = Vec::new();
        for generation in self.migration_index()?.generations.into_values() {
            let binding = &generation.binding;
            if !binding.ready {
                return Err(recovery_required());
            }
            if generation.state == crate::state::GenerationState::Candidate
                && let Some(owner) = &generation.repair_owner
                && !owner.absent()?
            {
                return Err(recovery_required());
            }
            let dir = self.binding_location(binding)?;
            recovery::layout::audit_profile(&dir)?;
            let lock = Lock::acquire_existing(&dir.join("auth.lock"))?;
            let identity = LockIdentity::from_fd(lock.fd())?;
            if generation.credential_lock.as_ref() != Some(&identity) {
                return Err(recovery_required());
            }
            recovery::migration_records(&dir, binding.id, &identity)?;
            native.push((dir.join("auth.lock"), lock));
        }
        native.extend(recovery::migration_probes(self)?);
        let owner = MigrationOwnership {
            native,
            daemon,
            root,
            directory: (self.root.clone(), metadata.dev(), metadata.ino()),
        };
        owner.check()?;
        Ok(owner)
    }
}

pub(in crate::state) fn unchanged_marker(lock: &Lock, path: &std::path::Path) -> Result<()> {
    if LockIdentity::from_fd(lock.fd())? != LockIdentity::at(path)? {
        return Err(corrupt());
    }
    Ok(())
}
