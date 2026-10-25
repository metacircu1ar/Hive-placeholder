//! Native conversation branching, separate from delegation and task lifetime.
pub(in crate::runtime) mod history;
mod native;
use super::{api::SpawnArgs, domain::Agent};
use crate::{
    config::{Config, ResolvedSpawn},
    error::{Error, Result},
};
pub(super) use native::verify_binary;
use serde::{Deserialize, Serialize};

pub(super) const MATERIALIZATION_SECONDS: u64 = 300;

super::domain::domain_enum!(BranchPhase {
    WaitingForSource => "waiting_for_source", ReadyToFork => "ready_to_fork",
    Forking => "forking", Bound => "bound", Failed => "failed", Uncertain => "uncertain",
});

#[derive(Clone, Copy, Debug)]
pub(crate) enum BranchFailure {
    BranchMaterializationTimeout,
    BranchLifetimeExpired,
    BranchTerminated,
    BranchStopped,
    BranchCompletedBeforeMaterialization,
    BranchCreationUncertain,
    BranchCancelled,
    SourceBoundaryUncertain,
    SourceIdentityChanged,
    SourceCleanupUncertain,
    BranchCancelledBeforeCreation,
    ForkNotSubmitted,
    NativeNotStarted,
    Error(crate::error::ErrorCode),
}
impl BranchFailure {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::BranchMaterializationTimeout => "branch_materialization_timeout",
            Self::BranchLifetimeExpired => "branch_lifetime_expired",
            Self::BranchTerminated => "branch_terminated",
            Self::BranchStopped => "branch_stopped",
            Self::BranchCompletedBeforeMaterialization => "branch_completed_before_materialization",
            Self::BranchCreationUncertain => "branch_creation_uncertain",
            Self::BranchCancelled => "branch_cancelled",
            Self::SourceBoundaryUncertain => "source_boundary_uncertain",
            Self::SourceIdentityChanged => "source_identity_changed",
            Self::SourceCleanupUncertain => "source_cleanup_uncertain",
            Self::BranchCancelledBeforeCreation => "branch_cancelled_before_creation",
            Self::ForkNotSubmitted => "fork_not_submitted",
            Self::NativeNotStarted => "native_not_started",
            Self::Error(code) => code.as_str(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FinishedTurn {
    pub thread: String,
    pub execution: String,
    pub turn: String,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub message: Option<String>,
    pub outcome: super::domain::ProviderTurnStatus,
    pub at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptedTurn {
    pub message: String,
    pub thread: String,
    pub execution: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    pub source: String,
    pub source_thread: String,
    pub source_execution: String,
    pub source_turn: String,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub phase: BranchPhase,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub deadline: Option<u64>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub source_outcome: Option<super::domain::ProviderTurnStatus>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub source_handoff: Option<String>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub native_thread: Option<String>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub error: Option<String>,
}
impl Intent {
    pub(super) fn initial_body(&self, child: &str, body: &str) -> String {
        format!(
            concat!(
                "Hive branch identity: You are agent {child}, a new conversation ",
                "branched from agent {} through native turn {}. Inherited history is ",
                "context only. The source continues independently. Do not continue ",
                "old commitments, correspondent replies or side effects unless the ",
                "new task explicitly requests them. Use your new identity for all ",
                "Hive coordination.\n\nNew task:\n{body}",
            ),
            self.source,
            self.source_turn,
            child = child,
            body = body
        )
    }
}

#[derive(Clone)]
pub(super) struct Source {
    pub agent: Agent,
    pub instructions: String,
}
impl Source {
    pub(super) fn fresh(&self, current: &Agent) -> bool {
        super::preflight::Identity::from(&self.agent) == super::preflight::Identity::from(current)
            && self.agent.desired_selection() == current.desired_selection()
            && self.agent.auth_group == current.auth_group
            && self.agent.sandbox == current.sandbox
            && self.agent.api_fallback == current.api_fallback
            && self.agent.handoff == current.handoff
            && effective_selection(&self.agent) == effective_selection(current)
    }

    pub(super) fn resolve(&self, config: &Config, args: &SpawnArgs) -> Result<ResolvedSpawn> {
        let options = args.options()?;
        let source = &self.agent;
        super::features::require(source)?;
        let effective = effective_selection(source);
        let inherited = effective.options.clone();
        let role = super::api::selection::role(config, options.role)?;
        for provider in [options.provider, role.and_then(|r| r.provider)]
            .into_iter()
            .flatten()
        {
            if provider != source.profile.provider {
                return Err(Error::binding_conflict(
                    "A native branch must use its source provider.",
                ));
            }
        }
        for auth in [options.auth, role.and_then(|r| r.auth.as_deref())]
            .into_iter()
            .flatten()
        {
            if auth != source.auth && source.auth_group.as_deref() != Some(auth) {
                return Err(Error::binding_conflict(
                    "A native branch must use its source login and credential generation.",
                ));
            }
        }
        let inherited_configs = source.agent_options.inherited_configs(&source.id);
        if options
            .sandbox
            .is_some_and(|sandbox| sandbox != source.sandbox)
        {
            return Err(Error::binding_conflict(
                "A branch must retain its source sandbox mode.",
            ));
        }
        let mut resolved = config.resolve_spawn(crate::config::SpawnOptions {
            options: options.options,
            inherited: Some((&inherited, &source.id)),
            inherited_configs: Some(&inherited_configs),
            config: options.config,
            role: options.role,
            provider: Some(source.profile.provider),
            auth: options
                .auth
                .or(role.and_then(|r| r.auth.as_deref()))
                .or(Some(source.auth_group.as_deref().unwrap_or(&source.auth))),
            model: options.model,
            effort: options.effort,
            sandbox: Some(source.sandbox),
            lifetime: options.lifetime,
            api_fallback: options
                .api_fallback
                .or(role.map(|r| r.api_fallback))
                .or(Some(source.api_fallback)),
        })?;
        if options.role.is_none() {
            resolved.comb.clone_from(&source.comb);
            resolved.mcp_servers.clone_from(&source.mcp_servers);
        }
        if source.profile.provider == crate::config::Provider::Codex
            && resolved.comb.as_ref().map(|b| b.reviewer)
                != source.comb.as_ref().map(|b| b.reviewer)
        {
            return Err(Error::binding_conflict(
                "Codex branches must retain the source Comb tool catalog; choose a role \
                    with the same Comb enablement and reviewer capability.",
            ));
        }
        Ok(resolved)
    }
}
pub(super) fn effective_selection(agent: &Agent) -> super::domain::OptionsSelection {
    agent
        .turn_selection
        .as_ref()
        .or(agent.last_accepted_selection.as_ref())
        .or(agent.session_selection.as_ref())
        .cloned()
        .unwrap_or_else(|| agent.desired_selection())
}
impl Agent {
    pub(super) fn branch_unbound(&self) -> bool {
        self.branch_phase
            .is_some_and(|phase| phase != BranchPhase::Bound)
    }
}

#[cfg(test)]
mod cli_tests;

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Controlled branch selection fixtures.
mod option_tests;
