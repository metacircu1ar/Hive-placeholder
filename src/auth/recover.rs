//! Credential ownership reconciliation and recovery.
use super::{
    Auth, AuthOperation, AuthOutcome, Error, RecoveryOutcome, Report, Result, Verification,
    verify_pending,
};
use crate::{
    runtime::api::value as project,
    state::{OperationKind, RecoveryReport},
};
use serde::Serialize;

#[derive(Serialize)]
#[serde(untagged)]
enum Recovery {
    Report(RecoveryReport),
    Failure(Failure),
}
impl Recovery {
    fn runtime_usable(&self) -> bool {
        match self {
            Self::Report(report) => report.runtime_usable(),
            Self::Failure(report) => report.runtime_usable,
        }
    }
    fn unavailable(diagnostic: Error) -> Self {
        Self::Failure(Failure {
            action: None,
            diagnostic,
            read_only: None,
            runtime_usable: false,
            snapshot_complete: false,
        })
    }
    fn lookup_failed(diagnostic: Error) -> Self {
        Self::Failure(Failure {
            action: Some(concat!(
                "Repair into a new controller with auth repair --reauthenticate ",
                "--new-state-dir; old state was not modified.",
            )),
            diagnostic,
            read_only: Some(true),
            runtime_usable: false,
            snapshot_complete: false,
        })
    }
}
#[derive(Serialize)]
struct Failure {
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<&'static str>,
    diagnostic: Error,
    #[serde(skip_serializing_if = "Option::is_none")]
    read_only: Option<bool>,
    runtime_usable: bool,
    snapshot_complete: bool,
}
#[derive(Serialize)]
struct RecoveryOutput<'a> {
    credential_verification: Option<Report>,
    operation: AuthOperation,
    outcome: AuthOutcome,
    profile: &'a str,
    recovery: Option<Recovery>,
}
impl Auth {
    pub(crate) fn recover(&self, name: &str) -> Result<RecoveryOutcome> {
        self.state.check_format()?;
        let profile = self.config.profile(name)?;
        let mut verification = None;
        let mut success = true;
        let recovery = match self.state.lookup(name, profile) {
            Ok(Some(binding)) => {
                if self.state.pending_credential_attempt(binding.id)?.is_some() {
                    let mut report = Report::new(
                        name,
                        profile,
                        crate::observability::optional(
                            self.binary(profile),
                            "recovery_binary_metadata",
                        ),
                    );
                    report.operation = AuthOperation::Recover;
                    report.generation = Some(binding.id);
                    let checked = self.recover_pending(&binding, profile, &mut report);
                    if let Err(error) = checked {
                        success = false;
                        report.verification_failed(&self.state, &binding, error);
                    }
                    verification = Some(report);
                }
                Some(
                    self.state
                        .recovery_report(&binding)
                        .map_or_else(Recovery::unavailable, Recovery::Report),
                )
            }
            Err(error @ Error::UnsupportedSchema { .. }) => return Err(error),
            Err(error) => {
                success = false;
                Some(Recovery::lookup_failed(error))
            }
            Ok(None) => None,
        };
        success &= recovery.as_ref().is_none_or(Recovery::runtime_usable);
        Ok(RecoveryOutcome {
            success,
            report: project(RecoveryOutput {
                operation: AuthOperation::Recover,
                profile: name,
                outcome: outcome(verification.as_ref(), success, recovery.is_some()),
                recovery,
                credential_verification: verification,
            }),
        })
    }
    fn recover_pending(
        &self,
        binding: &super::Binding,
        profile: &super::Profile,
        report: &mut Report,
    ) -> Result<()> {
        let mut operation = self.state.operation(binding, OperationKind::Recover)?;
        if !operation.has_pending()? {
            report.outcome = Some(AuthOutcome::AlreadyVerified);
            return operation.finish();
        }
        self.probe(
            profile,
            binding,
            &mut operation,
            report,
            self.settings.cancellation.clone(),
        );
        let verified = verify_pending(&operation, report);
        let finished = operation.finish();
        match (verified, finished) {
            (Err(primary), Err(secondary)) => {
                tracing::warn!(name: "auth.recovery_cleanup_failed",
                     primary = &primary as &(dyn std::error::Error + 'static),
                     secondary = &secondary as &(dyn std::error::Error + 'static),
                     decision = "preserve_verification_error");
                Err(primary)
            }
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }
}

fn outcome(verification: Option<&Report>, success: bool, recovery: bool) -> AuthOutcome {
    if verification.is_some_and(|report| report.outcome == Some(AuthOutcome::RepairRequired)) {
        AuthOutcome::RepairRequired
    } else if verification.is_some_and(|report| report.verification == Some(Verification::Pending))
    {
        AuthOutcome::VerificationPending
    } else if !success {
        AuthOutcome::RecoveryFailed
    } else if recovery {
        AuthOutcome::Reconciled
    } else {
        AuthOutcome::NoRecoveryNeeded
    }
}

#[cfg(test)]
mod tests;
