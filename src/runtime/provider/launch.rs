//! Provider-specific immutable process launch identity.
use super::claude::SessionConfig;
use crate::launch::LauncherCache;
use std::{process::Command, sync::Arc};

#[derive(Clone, Default)]
pub(crate) enum Backend {
    #[default]
    Codex,
    Claude(Arc<SessionConfig>),
}
impl Backend {
    pub(crate) fn multiplexed(&self) -> bool {
        matches!(self, Self::Codex)
    }
}
#[derive(Clone)]
pub(crate) struct LaunchOwner {
    pub agent: String,
    pub execution: String,
}
pub(crate) struct Launch {
    pub process_options: String,
    // Production worker launches carry preflight evidence; direct transport test
    // fixtures exercise the native protocol independently of version subprocesses.
    pub compatibility: Option<crate::provider_support::Evidence>,
    pub command: Command,
    pub launcher: Arc<LauncherCache>,
    pub backend: Backend,
    pub owner: Option<LaunchOwner>,
    pub credential: crate::auth::Credential,
}
#[cfg(test)]
impl Launch {
    pub(crate) fn codex(command: Command, launcher: Arc<LauncherCache>) -> Self {
        Self {
            process_options: String::new(),
            compatibility: None,
            command,
            launcher,
            backend: Backend::Codex,
            owner: None,
            credential: crate::auth::Credential::default(),
        }
    }
}
