//! Pruning admits retained audit state, never a replacement credential lock.
use super::{Binding, GenerationState, Lock, LockIdentity, State, corrupt, retirement};
use crate::{error::Result, state};

impl State {
    pub(in crate::state) fn verify_prune_lock(&self, binding: &Binding, lock: &Lock) -> Result<()> {
        let _metadata = self.metadata_lock()?;
        let dir = self.binding_location(binding)?;
        state::check_path(&self.root.join("profiles"), true)?;
        state::recovery::layout::audit_profile(&dir)?;
        let identity: Binding = state::read_json(&dir.join("identity.json"))?;
        let index = self.read_index()?;
        let generation = index.generation(binding.id)?;
        if !binding.ready
            || !generation.binding.ready
            || !identity.same_identity(binding)
            || !generation.binding.same_identity(binding)
            || index
                .profiles
                .values()
                .any(|active| active.id == binding.id)
            || !matches!(
                generation.state,
                GenerationState::Retired | GenerationState::Pruning | GenerationState::Pruned
            )
        {
            return Err(corrupt());
        }
        let lock_identity = LockIdentity::from_fd(lock.fd())?;
        if LockIdentity::at(&dir.join("auth.lock"))? != lock_identity
            || generation.credential_lock.as_ref() != Some(&lock_identity)
        {
            return Err(state::recovery_required());
        }
        retirement::validate(
            binding,
            &generation.profile_name,
            &lock_identity,
            generation.state,
        )
    }
}
