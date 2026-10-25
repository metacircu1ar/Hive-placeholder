//! Runtime format proof held by the same daemon owner as the validated store.
use super::{Shared, State};
use crate::{
    error::{Error, Result},
    state::{Lock, checked_metadata},
};
use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(in crate::state) struct Runtime {
    root: PathBuf,
    identities: [(u64, u64); 3],
    daemon: Arc<Lock>,
    pub(super) owner: Shared,
}

#[cfg(test)]
thread_local! {
    static CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn identity(path: &Path, directory: bool) -> Result<(u64, u64)> {
    let meta = checked_metadata(path, directory)?;
    Ok((meta.dev(), meta.ino()))
}
impl Runtime {
    pub(in crate::state) fn check(&self, state: &State) -> Result<()> {
        #[cfg(test)]
        CHECKS.set(CHECKS.get() + 1);
        self.owner.check().map_err(Error::state_io_from)?;
        crate::runtime::check_existing_paths(&state.root.join("runtime/runtime.sqlite"))?;
        if self.root != state.root
            || self.identities
                != [
                    identity(&state.root, true)?,
                    identity(&state.root.join("runtime/runtime.sqlite"), false)?,
                    identity(&state.root.join("runtime/daemon.lock"), false)?,
                ]
        {
            return Err(Error::unsafe_state("Runtime admission ownership changed."));
        }
        let held = crate::os::file_identity(self.daemon.fd()).map_err(Error::state_io_from)?;
        if (held.device, held.inode) != self.identities[2] {
            return Err(Error::unsafe_state("Runtime daemon ownership changed."));
        }
        // These small authoritative files may be externally corrupted even while
        // migration and maintenance are excluded. Preserve their refusal checks.
        super::super::format::check(&state.root)?;
        Ok(())
    }
}
impl State {
    /// Called only after the daemon opens and validates its Store. The retained
    /// daemon lock excludes ordinary maintenance; Shared alone is insufficient.
    pub(crate) fn for_runtime(mut self, daemon: Arc<Lock>) -> Result<Self> {
        let owner = self.admission()?;
        let held = daemon
            .admission()
            .ok_or_else(|| Error::internal_error("Missing daemon admission."))?;
        if !owner.same_owner(&held) {
            return Err(Error::unsafe_state(
                "Daemon admission belongs to another root.",
            ));
        }
        self.check_format_owned(&owner)?;
        crate::runtime::check_existing_format(&self.root.join("runtime/runtime.sqlite"))?;
        let proof = Arc::new(Runtime {
            identities: [
                identity(&self.root, true)?,
                identity(&self.root.join("runtime/runtime.sqlite"), false)?,
                identity(&self.root.join("runtime/daemon.lock"), false)?,
            ],
            root: self.root.clone(),
            daemon,
            owner,
        });
        proof.check(&self)?;
        self.runtime = Some(proof);
        Ok(self)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Private state and ownership fixtures fail at their assertion.
mod tests;
