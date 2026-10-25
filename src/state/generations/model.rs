//! Define immutable credential generation records and replacement identities.
use super::Binding;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GenerationState {
    Active,
    Candidate,
    Retired,
    Pruning,
    Pruned,
}

// These are independent durable facts (authentication, mutation uncertainty, and
// authentication uncertainty), not alternatives in the generation state machine.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Generation {
    pub binding: Binding,
    pub state: GenerationState,
    pub profile_name: String,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub replaces: Option<Uuid>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub credential_namespace: Option<String>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub account_identity: Option<String>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub key_fingerprint: Option<crate::config::KeyFingerprint>,
    pub key_revision: u64,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub credential_lock: Option<super::LockIdentity>,
    pub authenticated: bool,
    pub ever_authenticated: bool,
    pub credential_fence: bool,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub credential_mutation: Option<Uuid>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub credential_writer: Option<Uuid>,
    pub possibly_authenticated: bool,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub keychain_account: Option<String>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub repair_owner: Option<super::ProcessIdentity>,
}
impl Generation {
    pub(in crate::state) fn new(
        binding: Binding,
        name: &str,
        keychain_account: Option<String>,
    ) -> Self {
        Self {
            binding,
            state: GenerationState::Active,
            profile_name: name.into(),
            replaces: None,
            credential_namespace: None,
            account_identity: None,
            key_fingerprint: None,
            key_revision: 0,
            credential_lock: None,
            authenticated: false,
            ever_authenticated: false,
            credential_fence: false,
            credential_mutation: None,
            credential_writer: None,
            possibly_authenticated: false,
            keychain_account,
            repair_owner: None,
        }
    }
}

pub struct Repair {
    pub(super) _admission: crate::state::admission::Shared,
    pub binding: Binding,
    pub expected: Uuid,
    pub name: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PendingCredentialAttempt {
    pub run: Uuid,
    pub writer: Uuid,
}
