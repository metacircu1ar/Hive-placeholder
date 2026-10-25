//! Shared non-secret freshness tokens; the index remains the durable authority.
use super::{Credential, credential::Selection};
use crate::{
    error::{Error, Result},
    state::{Binding, State},
};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Default)]
pub(crate) struct Keys(Mutex<HashMap<uuid::Uuid, Weak<AtomicU64>>>);
impl Keys {
    pub(crate) fn resolve(&self, state: &State, binding: &Binding) -> Result<Credential> {
        if binding.profile.method == crate::config::Method::Subscription {
            return Ok(Credential::Subscription);
        }
        // Serialize daemon selections across filesystem/catalog workers. Writer
        // admission only reads the returned atomic; it never takes this lock.
        let mut keys = self.0.lock().map_err(|cause| {
            drop(cause);
            Error::internal_error("API key selection owner failed.")
                .with_source(std::sync::PoisonError::new(()))
        })?;
        keys.retain(|_, value| value.strong_count() != 0);
        let current = keys
            .get(&binding.id)
            .and_then(Weak::upgrade)
            .unwrap_or_default();
        keys.insert(binding.id, Arc::downgrade(&current));
        let observed = state.key_authority(binding)?;
        let mut credential = Credential::resolve(&binding.profile)?;
        let fingerprint = credential
            .fingerprint()
            .ok_or_else(|| Error::internal_error("API key snapshot is missing."))?;
        // A changed selection may become visible and then fail its durability
        // barrier. Revoke before that boundary and keep it revoked on error.
        // Reconfirming an unchanged authority must not invalidate proofs that
        // already passed durability; a prior failed rotation still has token 0.
        if observed
            .as_ref()
            .is_none_or(|authority| authority.fingerprint != fingerprint)
        {
            current.store(0, Ordering::Release);
        }
        let authority = state.select_key(binding, observed.as_ref(), &fingerprint)?;
        current.fetch_max(authority.revision.get(), Ordering::AcqRel);
        if let Credential::ApiKey(_, selection) = &mut credential {
            *selection = Some(Selection { authority, current });
        }
        Ok(credential)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    mod publication;
    #[test]
    fn changed_source_invalidates_pending_proofs_but_keeps_captured_authority() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("key");
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .write(&path, b"first")
            .unwrap();
        let state = State::new(root.join("state")).unwrap();
        let binding = state
            .register(
                "api",
                &crate::config::Profile {
                    provider: crate::config::Provider::Codex,
                    method: crate::config::Method::ApiKey,
                    api_key_env: None,
                    api_key_file: Some(path.clone()),
                },
            )
            .unwrap();
        let keys = Keys::default();
        let first = keys.resolve(&state, &binding).unwrap();
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .write(&path, b"second")
            .unwrap();
        assert!(crate::state::is_key_changed(
            &first.verify_source(&state, &binding).unwrap_err()
        ));
        let second = keys.resolve(&state, &binding).unwrap();
        assert!(!first.fresh());
        assert!(second.fresh());
        assert_eq!(first.authority().unwrap().revision.get(), 1);
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .write(&path, b"first")
            .unwrap();
        let third = keys.resolve(&state, &binding).unwrap();
        assert!(!first.fresh());
        assert!(!second.fresh());
        assert!(third.fresh());
        assert_eq!(third.fingerprint(), first.fingerprint());
        assert_ne!(third.authority(), first.authority());
    }
}
