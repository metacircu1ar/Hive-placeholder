//! Subscription selection and durable boundary switches; no implicit billing changes.
pub(in crate::runtime) use crate::runtime::preflight::auth as prepare;
use serde::{Deserialize, Serialize};
mod admission;
mod availability;
mod cutoff;
mod operations;
mod probes;
mod restore;
pub(in crate::runtime) mod selection;
pub(in crate::runtime) use selection::CatalogMiss;
pub(super) use selection::{Selection, SelectionRequest, select};
mod service;
pub(in crate::runtime) mod signals;
mod switch;
pub use availability::{Availability, AvailabilityState, Probe};
pub(in crate::runtime) use service::run;

use super::domain::domain_enum;
domain_enum!(WaitingReason { Quota => "quota", Switch => "switch" });
domain_enum!(AuthReason {
    UsageLimit => "usage_limit",
    Authentication => "authentication",
    UsageAndAuthentication => "usage_and_authentication",
    SpendCap => "spend_cap",
    Manual => "manual",
});
domain_enum!(AuthChoice {
    Ordered => "ordered",
    Balanced => "balanced",
    Sticky => "sticky",
    LimitedMove => "limited_move",
    PaidTail => "paid_tail",
    Waiting => "waiting",
});
domain_enum!(ProbeError {
    MissingOwner => "missing_owner",
    OwnerBindingMismatch => "owner_binding_mismatch",
    MissingInput => "missing_input",
});

/// The last actual group selection for an agent; direct profiles have none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthDecision {
    pub mode: crate::config::AuthMode,
    pub reason: AuthChoice,
}
impl AuthDecision {
    pub(in crate::runtime) fn new(group: &crate::config::AuthGroup, reason: AuthChoice) -> Self {
        Self {
            mode: group.mode,
            reason,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwitchIntent {
    pub target: String,
    pub reason: AuthReason,
    pub source_generation: uuid::Uuid,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub source_execution: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod recovery_tests;

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Corruption fixtures fail immediately on setup errors.
mod records;

#[cfg(test)]
mod key_tests;

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Fixtures stop immediately on setup failure.
mod balance_tests;
