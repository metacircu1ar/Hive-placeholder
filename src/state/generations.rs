//! Select and verify immutable credential generations.
mod claude_namespace;
mod credential;
mod key;
pub use key::KeyAuthority;
pub(crate) use key::{is_key_changed, key_changed};
mod model;
mod repair;

pub(super) use claude_namespace::claude_keychain_account;
pub use claude_namespace::claude_namespace;
pub(crate) use model::PendingCredentialAttempt;
pub use model::{Generation, GenerationState, Repair};

use super::{
    Binding, Lock, LockIdentity, ProcessIdentity, State, corrupt, read_json, recovery,
    recovery_required,
};
use crate::{
    config::Profile,
    error::{Error, Result},
};
use uuid::Uuid;

impl State {
    pub fn metadata_lock(&self) -> Result<Lock> {
        let admission = self.format_admission()?;
        Ok(Lock::index(&self.root.join("index.lock"))?.with_admission(admission))
    }

    /// Select under the metadata transaction. Runtime callers release this lock
    /// before writer admission; the daemon maintenance owner excludes pruning
    /// through committed pins and completion of its joined preflight work.
    pub fn selected_generation(&self, name: &str, profile: &Profile) -> Result<Binding> {
        let binding = self
            .read_index()?
            .profiles
            .remove(name)
            .ok_or(Error::login_required(
                "Run auth login for this profile first.",
            ))?;
        if &binding.profile != profile {
            return Err(corrupt());
        }
        self.validate_binding(&binding)?;
        Ok(binding)
    }

    pub fn generation(&self, name: &str, profile: &Profile, id: Uuid) -> Result<Binding> {
        let _metadata = self.metadata_lock()?;
        let generation = self.read_index()?.into_generation(id)?;
        if &generation.binding.profile != profile
            || generation.profile_name != name
            || !matches!(
                generation.state,
                GenerationState::Active | GenerationState::Retired
            )
        {
            return Err(Error::recovery_required(concat!(
                "Agent credential generation is unavailable; inspect its history or ",
                "create a new agent.",
            )));
        }
        self.validate_binding(&generation.binding)?;
        Ok(generation.binding)
    }

    pub fn generations(&self, name: &str) -> Result<Vec<Generation>> {
        let _metadata = self.metadata_lock()?;
        let index = self.read_index()?;
        let active = index.profiles.get(name).map(|b| b.id);
        Ok(index
            .generations
            .into_values()
            .filter(|g| Some(g.binding.id) == active || g.profile_name == name)
            .collect())
    }
}

impl State {
    /// Verify an immutable namespace without consulting a cached active pointer.
    pub fn verify_generation(&self, binding: &Binding) -> Result<()> {
        let identity: Binding = read_json(&binding.directory.join("identity.json"))?;
        if !identity.same_identity(binding) {
            return Err(corrupt());
        }
        self.validate_binding(binding)
    }
}
