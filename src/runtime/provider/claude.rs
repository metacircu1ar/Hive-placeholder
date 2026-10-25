//! Native Claude Code stream-JSON protocol without an SDK sidecar.
use crate::{
    error::{Error, Result},
    process::{Cancellation, Captured},
    state::Operation,
};
use serde::Deserialize;
use serde_json::Value;
use std::{process::Command, time::Duration};
mod codec;
mod discovery;
pub(super) use discovery::catalog;
pub(crate) use discovery::discover;
mod launch;
mod read_only_git;
#[cfg(test)]
mod tests;
pub(crate) mod wire;
pub(super) use codec::Codec;
pub(in crate::runtime) use codec::user_frame;
pub(crate) use codec::{MAX_HELD, Permit as SteeringPermit, ReceiptMode};
pub(crate) use launch::{History, IdentityPrompt, SessionConfig};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Model {
    pub value: String,
    #[serde(rename = "resolvedModel")]
    pub resolved: String,
    pub display_name: String,
    pub description: String,
    #[serde(default)]
    pub supported_effort_levels: Vec<String>,
}

#[derive(Deserialize)]
pub(super) struct Initialization {
    pub models: Vec<Model>,
    pub account: Account,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Account {
    email: Option<String>,
    subscription_type: Option<String>,
    api_provider: wire::ApiProvider,
    api_key_source: Option<wire::ApiKeySource>,
}
impl Account {
    pub(crate) fn matches(&self, method: crate::config::Method) -> bool {
        match method {
            crate::config::Method::Subscription => self.subscription(),
            crate::config::Method::ApiKey => {
                self.api_provider == wire::ApiProvider::FirstParty
                    && self.api_key_source == Some(wire::ApiKeySource::SelectedEnvironment)
            }
        }
    }
    pub(crate) fn subscription(&self) -> bool {
        self.api_provider == wire::ApiProvider::FirstParty
            && self
                .subscription_type
                .as_ref()
                .is_some_and(|s| !s.is_empty())
            && self
                .api_key_source
                .is_none_or(|s| s == wire::ApiKeySource::None)
    }
}
