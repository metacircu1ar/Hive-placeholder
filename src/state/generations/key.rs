//! API selection counters are independent of credential ownership and authentication.
use super::{Binding, State, corrupt};
use crate::{
    config::KeyFingerprint,
    error::{Error, Result},
};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

#[derive(Debug, thiserror::Error)]
#[error("API selection changed before input submission")]
struct KeyChanged;
pub(crate) fn key_changed() -> Error {
    Error::agent_busy("API key changed during preflight; retry before submitting input.")
        .with_source(KeyChanged)
}

pub(crate) fn is_key_changed(error: &Error) -> bool {
    std::error::Error::source(error).is_some_and(<dyn std::error::Error>::is::<KeyChanged>)
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KeyAuthority {
    pub revision: NonZeroU64,
    pub fingerprint: KeyFingerprint,
}

impl super::Generation {
    pub(crate) fn key_authority(&self) -> Result<Option<KeyAuthority>> {
        match (&self.key_fingerprint, NonZeroU64::new(self.key_revision)) {
            (None, None) => Ok(None),
            (Some(fingerprint), Some(revision)) => Ok(Some(KeyAuthority {
                revision,
                fingerprint: fingerprint.clone(),
            })),
            _ => Err(corrupt()),
        }
    }
}

impl State {
    pub(crate) fn key_authority(&self, binding: &Binding) -> Result<Option<KeyAuthority>> {
        self.update_index(|index| {
            let generation = index.generation(binding.id)?;
            if !generation.binding.same_identity(binding) {
                return Err(corrupt());
            }
            generation.key_authority()
        })
    }

    /// The source was read outside this transaction. A racing selection must be
    /// retried from that read, never applied over a newer selection (including A→B→A).
    pub(crate) fn select_key(
        &self,
        binding: &Binding,
        observed: Option<&KeyAuthority>,
        fingerprint: &KeyFingerprint,
    ) -> Result<KeyAuthority> {
        self.update_index(|index| {
            let generation = index.generation_mut(binding.id)?;
            if !generation.binding.same_identity(binding)
                || binding.profile.method != crate::config::Method::ApiKey
                || !matches!(
                    generation.state,
                    super::GenerationState::Active | super::GenerationState::Retired
                )
            {
                return Err(corrupt());
            }
            let current = generation.key_authority()?;
            if current.as_ref() != observed {
                return Err(key_changed());
            }
            if let Some(current) = current.filter(|value| &value.fingerprint == fingerprint) {
                // Visible metadata can be a prior replacement whose final sync
                // failed. Repeat checked publication before granting authority.
                index.save()?;
                return Ok(current);
            }
            let revision = generation
                .key_revision
                .checked_add(1)
                .and_then(NonZeroU64::new)
                .ok_or_else(corrupt)?;
            generation.key_revision = revision.get();
            generation.key_fingerprint = Some(fingerprint.clone());
            index.save()?;
            Ok(KeyAuthority {
                revision,
                fingerprint: fingerprint.clone(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_selection_rejects_stale_cas_and_preserves_aba_revision_after_restart() {
        let temp = tempfile::tempdir().unwrap();
        let state = State::new(temp.path().canonicalize().unwrap().join("state")).unwrap();
        let binding = state
            .register(
                "api",
                &crate::config::Profile {
                    provider: crate::config::Provider::Codex,
                    method: crate::config::Method::ApiKey,
                    api_key_env: Some("PRIVATE_KEY_TEST".into()),
                    api_key_file: None,
                },
            )
            .unwrap();
        let a = KeyFingerprint::digest(b"first");
        let b = KeyFingerprint::digest(b"second");
        let first = state.select_key(&binding, None, &a).unwrap();
        assert_eq!(first.revision.get(), 1);
        let second = state.select_key(&binding, Some(&first), &b).unwrap();
        assert!(is_key_changed(
            &state.select_key(&binding, Some(&first), &a).unwrap_err()
        ));
        let state = State::new(state.root).unwrap();
        let third = state.select_key(&binding, Some(&second), &a).unwrap();
        assert_eq!(third.fingerprint, first.fingerprint);
        assert_eq!(third.revision.get(), 3);
        assert_eq!(state.select_key(&binding, Some(&third), &a).unwrap(), third);
        let generation = state.generations("api").unwrap().remove(0);
        assert!(!generation.credential_fence);
        assert!(!generation.authenticated);
        assert!(generation.account_identity.is_none());
        assert_eq!(generation.key_authority().unwrap(), Some(third));
    }
}
