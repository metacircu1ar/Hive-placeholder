//! One resolved secret snapshot; only the native transport can serialize its key.
use super::key::{self, Secret};
use crate::{
    config::{Method, Profile, Provider},
    error::{Error, Result},
};
use serde::{Serialize, Serializer, ser::SerializeStruct};
use std::{
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone)]
pub(crate) struct Selection {
    pub authority: crate::state::KeyAuthority,
    pub current: Arc<AtomicU64>,
}

#[derive(Clone, Default)]
pub(crate) enum Credential {
    #[default]
    Subscription,
    ApiKey(Arc<Secret>, Option<Selection>),
}
impl Credential {
    pub(crate) fn fingerprint(&self) -> Option<crate::config::KeyFingerprint> {
        match self {
            Self::Subscription => None,
            Self::ApiKey(key, _) => Some(crate::config::KeyFingerprint::digest(key.0.as_bytes())),
        }
    }
    pub(crate) fn resolve(profile: &Profile) -> Result<Self> {
        profile.validate()?;
        match profile.method {
            Method::Subscription => Ok(Self::Subscription),
            Method::ApiKey => Ok(Self::ApiKey(Arc::new(key::resolve(profile)?), None)),
        }
    }
    pub(crate) fn method(&self) -> Method {
        match self {
            Self::Subscription => Method::Subscription,
            Self::ApiKey(..) => Method::ApiKey,
        }
    }
    pub(crate) fn configure(&self, provider: Provider, command: &mut Command) {
        if let Self::ApiKey(key, _) = self {
            match provider {
                Provider::Codex => {
                    // Last command-line override wins; managed requirements are
                    // independently checked before transmitting the secret.
                    command.args([
                        "-c",
                        "cli_auth_credentials_store=\"ephemeral\"",
                        "-c",
                        "model_provider=\"openai\"",
                    ]);
                }
                Provider::Claude => {
                    command.env("ANTHROPIC_API_KEY", &key.0);
                }
            }
        }
    }
    pub(crate) fn login(&self) -> Result<ApiLogin> {
        match self {
            Self::ApiKey(key, _) => Ok(ApiLogin(key.clone())),
            Self::Subscription => Err(Error::policy_conflict(
                "API login requires a selected API key.",
            )),
        }
    }
    pub(crate) fn authority(&self) -> Option<&crate::state::KeyAuthority> {
        match self {
            Self::ApiKey(_, Some(selection)) => Some(&selection.authority),
            _ => None,
        }
    }
    pub(crate) fn fresh(&self) -> bool {
        match self {
            Self::ApiKey(_, Some(selection)) => {
                selection.current.load(Ordering::Acquire) == selection.authority.revision.get()
            }
            _ => true,
        }
    }
    /// Called only outside the writer after catalog work, before admission.
    pub(crate) fn verify_source(
        &self,
        state: &crate::state::State,
        binding: &crate::state::Binding,
    ) -> Result<()> {
        if self.method() == Method::Subscription {
            return Ok(());
        }
        if !self.fresh()
            || Self::resolve(&binding.profile)?.fingerprint() != self.fingerprint()
            || (self.authority().is_some()
                && state.key_authority(binding)?.as_ref() != self.authority())
        {
            return Err(crate::state::key_changed());
        }
        Ok(())
    }
}

/// Dedicated wire parameters. The underlying Secret has no Serialize or formatting trait.
#[derive(Clone)]
pub(crate) struct ApiLogin(Arc<Secret>);
impl Serialize for ApiLogin {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut value = serializer.serialize_struct("ApiLogin", 2)?;
        value.serialize_field("type", "apiKey")?;
        value.serialize_field("apiKey", &self.0.0)?;
        value.end()
    }
}
