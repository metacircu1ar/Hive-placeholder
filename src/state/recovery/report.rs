//! Typed recovery projections; JSON values are produced only at serialization boundaries.
use super::{
    Error, LockIdentity, Uuid,
    ledger::{Class, ExecutionPhase},
};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum RecoveryReport {
    Snapshot(Reconciliation),
    Diagnostic(Unavailable),
}
impl RecoveryReport {
    pub(crate) fn runtime_usable(&self) -> bool {
        match self {
            Self::Snapshot(report) => report.runtime_usable,
            Self::Diagnostic(report) => report.runtime_usable,
        }
    }
    pub(super) fn diagnostic(generation: Uuid, diagnostic: Error) -> Self {
        Self::Diagnostic(Unavailable {
            action: concat!(
                "Use auth repair --reauthenticate for isolated credentials; use --new-",
                "state-dir if controller ownership is damaged. No old evidence was ",
                "changed.",
            ),
            diagnostic,
            executions: [],
            generation,
            read_only: true,
            runtime_usable: false,
            snapshot_complete: false,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct Reconciliation {
    pub(super) account_identity_available: bool,
    pub(super) checked_at: u64,
    pub(super) credential_lock: LockIdentity,
    pub(super) executions: Vec<Observation>,
    pub(super) generation: Uuid,
    pub(super) history_error: Option<Error>,
    pub(super) runtime_usable: bool,
    pub(super) snapshot_current: bool,
}
#[derive(Debug, Serialize)]
pub struct Unavailable {
    action: &'static str,
    diagnostic: Error,
    executions: [(); 0],
    generation: Uuid,
    read_only: bool,
    runtime_usable: bool,
    snapshot_complete: bool,
}

#[derive(Debug, Serialize)]
pub(super) struct Observation {
    pub(super) class: Class,
    #[serde(flatten)]
    pub(super) completion: Completion,
    pub(super) conversation_safe: bool,
    pub(super) execution_id: Uuid,
    pub(super) ownership_gone: bool,
    pub(super) phase: ExecutionPhase,
    pub(super) revision: u64,
}
#[derive(Debug, Serialize)]
pub(super) struct Completion {
    pub(super) confirmed: bool,
    pub(super) quiescent: bool,
}

#[cfg(test)]
mod tests;
