//! Cold cleanup only after a non-reader operation has proven exclusive ownership.
use super::{Binding, Lock, LockIdentity, Result, State, exists, layout};
use crate::state::{
    self, corrupt,
    publication::cleanup::{ownership, sweep},
};

const LEAVES: [&str; 5] = [
    "executions",
    "writers",
    "execution-archive",
    "recovery-receipts",
    "credential-resolutions",
];

/// The caller has completed all credential and execution admission checks and
/// retains the original exclusive auth descriptor. Shared readers never call this.
pub(super) fn profile(state: &State, binding: &Binding, credential: &Lock) -> Result<()> {
    let dir = state.binding_location(binding)?;
    let check_credential = || ownership(credential, &dir.join("auth.lock"));
    check_credential()?;
    layout::audit_profile(&dir)?;
    {
        // Initialization publishes identity.json under index.lock; prune records
        // use exclusive auth ownership. Never nest evidence.lock inside index.lock.
        let index = state.metadata_lock()?;
        let snapshot = state.read_index()?;
        let generation = snapshot.generation(binding.id)?;
        let identity: Binding = state::read_json(&dir.join("identity.json"))?;
        if !binding.ready
            || !generation.binding.ready
            || !generation.binding.same_identity(binding)
            || !identity.same_identity(binding)
            || generation.credential_lock.as_ref() != Some(&LockIdentity::from_fd(credential.fd())?)
        {
            return Err(corrupt());
        }
        sweep(&dir, || {
            check_credential()?;
            ownership(&index, &state.root.join("index.lock"))
        })?;
    }
    let mut leaves = Vec::new();
    for name in LEAVES {
        let path = dir.join(name);
        if exists(&path)? {
            leaves.push(path);
        }
    }
    if leaves.is_empty() {
        return Ok(());
    }
    let evidence = Lock::index(&dir.join("evidence.lock"))?;
    for path in leaves {
        sweep(&path, || {
            check_credential()?;
            ownership(&evidence, &dir.join("evidence.lock"))
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
