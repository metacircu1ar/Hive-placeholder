//! Strict subscription groups with an explicit, ordered API-key tail.
use super::{Config, Method, Provider, valid_name};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    Ordered,
    Balanced,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthGroup {
    pub mode: AuthMode,
    pub members: Vec<AuthMember>,
    #[serde(default)]
    pub paid_tail: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum AuthMember {
    Name(String),
    Weighted(WeightedMember),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WeightedMember {
    pub profile: String,
    #[serde(
        default,
        deserialize_with = "weight",
        skip_serializing_if = "Option::is_none"
    )]
    pub weight: Option<u16>,
}

fn weight<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Option<u16>, D::Error> {
    u16::deserialize(d).map(Some)
}

impl AuthMember {
    pub fn profile(&self) -> &str {
        match self {
            Self::Name(name) => name,
            Self::Weighted(member) => &member.profile,
        }
    }
    pub fn weight(&self) -> u16 {
        match self {
            Self::Name(_) => 1,
            Self::Weighted(member) => member.weight.unwrap_or(1),
        }
    }
    fn explicit_weight(&self) -> bool {
        matches!(self, Self::Weighted(member) if member.weight.is_some())
    }
}

impl AuthGroup {
    pub fn profiles(&self) -> impl Iterator<Item = &str> {
        self.members
            .iter()
            .map(AuthMember::profile)
            .chain(self.paid_tail.iter().map(String::as_str))
    }
    pub fn contains(&self, profile: &str) -> bool {
        self.profiles().any(|member| member == profile)
    }
    /// A subscription member's balancing weight; other profiles weigh 1.
    pub fn weight(&self, profile: &str) -> u16 {
        self.members
            .iter()
            .find(|member| member.profile() == profile)
            .map_or(1, AuthMember::weight)
    }
    pub fn balanced(&self) -> bool {
        self.mode == AuthMode::Balanced
    }
    #[cfg(test)]
    pub(crate) fn ordered(members: Vec<String>) -> Self {
        Self {
            mode: AuthMode::Ordered,
            members: members.into_iter().map(AuthMember::Name).collect(),
            paid_tail: Vec::new(),
        }
    }
    #[cfg(test)]
    pub(crate) fn weighted(members: &[(&str, u16)]) -> Self {
        Self {
            mode: AuthMode::Balanced,
            members: members
                .iter()
                .map(|(profile, weight)| {
                    AuthMember::Weighted(WeightedMember {
                        profile: (*profile).to_owned(),
                        weight: Some(*weight),
                    })
                })
                .collect(),
            paid_tail: Vec::new(),
        }
    }
}

impl Config {
    pub(super) fn validate_auth_group(&self, name: &str, group: &AuthGroup) -> Result<()> {
        if !valid_name(name) || self.auth_profiles.contains_key(name) || group.members.is_empty() {
            return Err(Error::config_invalid(
                "Auth groups need unique names and a nonempty subscription members list; \
                    use mode: ordered or balanced.",
            ));
        }
        let mut seen = BTreeSet::new();
        let mut provider: Option<Provider> = None;
        for name in group.profiles() {
            let profile = self.profile(name)?;
            if !seen.insert(name) || provider.is_some_and(|p| p != profile.provider) {
                return Err(Error::config_invalid(
                    "Auth group members and paid_tail must be unique profiles of one provider.",
                ));
            }
            provider = Some(profile.provider);
        }
        for member in &group.members {
            if self.profile(member.profile())?.method != Method::Subscription {
                return Err(Error::config_invalid(
                    "Auth group members must use subscriptions; put API-key profiles in \
                        paid_tail: [api-profile].",
                ));
            }
            if !(1..=1000).contains(&member.weight()) {
                return Err(Error::config_invalid(
                    "Balanced member weights must be integers from 1 to 1000; for example \
                        {profile: plan, weight: 4}.",
                ));
            }
            if group.mode == AuthMode::Ordered && member.explicit_weight() {
                return Err(Error::config_invalid(
                    "Ordered groups refuse explicit weights; use members: [plan] or mode: \
                        balanced.",
                ));
            }
        }
        for profile in &group.paid_tail {
            if self.profile(profile)?.method != Method::ApiKey {
                return Err(Error::config_invalid(
                    "Auth group paid_tail must contain only API-key profiles; put \
                        subscriptions in members.",
                ));
            }
        }
        Ok(())
    }

    /// Ephemeral flat membership view for quota-wakeup SQL, never a config reader.
    pub(crate) fn auth_group_members(&self) -> BTreeMap<&str, Vec<&str>> {
        self.auth_groups
            .iter()
            .map(|(name, group)| (name.as_str(), group.profiles().collect()))
            .collect()
    }

    /// Offline configuration evidence. Runtime observations require the daemon.
    pub(crate) fn auth_group_summary(&self) -> BTreeMap<&str, serde_json::Value> {
        self.auth_groups
            .iter()
            .map(|(name, group)| {
                let members: Vec<_> = group
                    .members
                    .iter()
                    .map(|member| OfflineMember {
                        profile: member.profile(),
                        bound_agents: None,
                        active_turns: None,
                        weight: group.balanced().then(|| member.weight()),
                    })
                    .collect();
                (
                    name.as_str(),
                    crate::runtime::api::value(OfflineGroup {
                        mode: group.mode,
                        members,
                        paid_tail: &group.paid_tail,
                        load_available: false,
                    }),
                )
            })
            .collect()
    }
}

#[derive(Serialize)]
struct OfflineMember<'a> {
    profile: &'a str,
    bound_agents: Option<u64>,
    active_turns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    weight: Option<u16>,
}

#[derive(Serialize)]
struct OfflineGroup<'a> {
    mode: AuthMode,
    members: Vec<OfflineMember<'a>>,
    paid_tail: &'a [String],
    load_available: bool,
}

#[cfg(test)]
mod tests;
