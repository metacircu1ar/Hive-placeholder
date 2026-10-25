//! Configured spawn choices for a human spawn dialog.
use crate::config::{Config, Provider, SpawnOptions};
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct RolesResponse<'a> {
    /// The provider a spawn without a role or login uses, if the config names one.
    pub default_provider: Option<Provider>,
    /// Roles, which are also the spawn presets.
    pub roles: Vec<SpawnChoice<'a>>,
    pub profiles: Vec<SpawnChoice<'a>>,
    pub groups: Vec<SpawnChoice<'a>>,
}

#[derive(Serialize)]
pub(crate) struct SpawnChoice<'a> {
    pub name: &'a str,
    /// The provider a spawn with this choice alone uses; null when it does not resolve.
    pub provider: Option<Provider>,
}

impl<'a> RolesResponse<'a> {
    pub(crate) fn new(config: &'a Config) -> Self {
        let role = |name: &'a str| SpawnChoice {
            name,
            provider: config
                .spawn_provider_for(&SpawnOptions {
                    role: Some(name),
                    ..SpawnOptions::default()
                })
                .ok(),
        };
        Self {
            default_provider: config.spawn_provider_for(&SpawnOptions::default()).ok(),
            roles: config.roles.keys().map(|name| role(name)).collect(),
            profiles: config
                .auth_profiles
                .iter()
                .map(|(name, profile)| SpawnChoice {
                    name,
                    provider: Some(profile.provider),
                })
                .collect(),
            groups: config
                .auth_groups
                .keys()
                .map(|name| SpawnChoice {
                    name,
                    provider: config.auth_provider(name).ok(),
                })
                .collect(),
        }
    }
}
