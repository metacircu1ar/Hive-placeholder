//! Authentication status and outcome records.
use super::{
    Binding, Error, Method, Operation, PathBuf, Profile, Provider, Result, Serialize, State, state,
};
use crate::runtime::domain::domain_enum;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, serde::Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Missing,
    PresentUnverified,
    Authenticated,
    ReauthenticationRequired,
    MethodConflict,
    Unavailable,
    ProfileBusy,
    RecoveryRequired,
}

#[derive(Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Verification {
    Pending,
    Fenced,
}

domain_enum!(AuthOutcome {
    AlreadyAuthenticated => "already_authenticated",
    AlreadyVerified => "already_verified",
    Authenticated => "authenticated",
    Cancelled => "cancelled",
    GenerationReplaced => "generation_replaced",
    LoginFailed => "login_failed",
    LoginRefused => "login_refused",
    LoginSelectionUnknown => "login_selection_unknown",
    LoginSuperseded => "login_superseded",
    NoRecoveryNeeded => "no_recovery_needed",
    Reconciled => "reconciled",
    Registered => "registered",
    RecoveryFailed => "recovery_failed",
    RepairFinalizationFailed => "repair_finalization_failed",
    RepairRefused => "repair_refused",
    RepairRequired => "repair_required",
    RepairRetired => "repair_retired",
    RepairSuperseded => "repair_superseded",
    Timeout => "timeout",
    VerificationFailed => "verification_failed",
    VerificationPending => "verification_pending",
});

domain_enum!(AuthOperation {
    Status => "status",
    List => "list",
    Login => "login",
    Recover => "recover",
    Repair => "repair",
});

domain_enum!(ObservedMethod {
    Subscription => "subscription",
    Other => "other",
});

#[derive(Serialize)]
pub(crate) struct Report {
    pub(crate) operation: AuthOperation,
    pub(crate) profile: String,
    pub(crate) provider: Provider,
    pub(crate) method: Method,
    pub(crate) configuration_valid: bool,
    pub(crate) state: Status,
    pub(crate) credential_present: Option<bool>,
    pub(crate) observed_method: Option<ObservedMethod>,
    pub(crate) api_key_env: Option<String>,
    pub(crate) api_key_file: Option<PathBuf>,
    pub(crate) provider_available: bool,
    pub(crate) provider_executable: Option<PathBuf>,
    pub(crate) provider_version: Option<String>,
    pub(crate) provider_registry: crate::provider_support::Registry,
    pub(crate) provider_support: Option<crate::provider_support::Observation>,
    pub(crate) environment_source: &'static str,
    pub(crate) checked_at_unix_seconds: u64,
    pub(crate) diagnostic: Option<Error>,
    pub(crate) warnings: Vec<Error>,
    pub(crate) outcome: Option<AuthOutcome>,
    pub(crate) state_directory: Option<PathBuf>,
    pub(crate) retired_controller: Option<PathBuf>,
    pub(crate) generation: Option<Uuid>,
    pub(crate) account_identity: Option<String>,
    pub(crate) verification: Option<Verification>,
    pub(crate) metadata_probe_directory: Option<PathBuf>,
}
impl Report {
    pub(super) fn new(name: &str, profile: &Profile, binary: Option<PathBuf>) -> Self {
        Self {
            operation: AuthOperation::Status,
            profile: name.into(),
            provider: profile.provider,
            method: profile.method,
            configuration_valid: true,
            state: Status::Missing,
            credential_present: None,
            observed_method: None,
            api_key_env: profile.api_key_env.clone(),
            api_key_file: profile.api_key_file.clone(),
            provider_available: binary.is_some(),
            provider_executable: binary,
            provider_version: None,
            provider_registry: crate::provider_support::registry(profile.provider),
            provider_support: None,
            environment_source: "cli",
            checked_at_unix_seconds: state::now(),
            diagnostic: None,
            warnings: Vec::new(),
            outcome: None,
            state_directory: None,
            retired_controller: None,
            generation: None,
            account_identity: None,
            verification: None,
            metadata_probe_directory: None,
        }
    }
    pub(super) fn fail(&mut self, error: Error) {
        tracing::warn!(name: "auth.operation_failed",
             phase = self.operation.as_str(),
             code = error.code(),
             error = &error as &(dyn std::error::Error + 'static),
             decision = "report_unavailable");
        self.credential_present = None;
        self.observed_method = None;
        self.state = match &error {
            Error::ProfileBusy { .. } => Status::ProfileBusy,
            Error::RecoveryRequired { .. } => Status::RecoveryRequired,
            _ => Status::Unavailable,
        };
        self.diagnostic = Some(error);
    }
    pub(super) fn verification_failed(&mut self, state: &State, binding: &Binding, error: Error) {
        self.fail(error);
        let fenced = match state.credential_fenced(binding) {
            Ok(fenced) => fenced,
            Err(error) => {
                tracing::warn!(name: "auth.fence_lookup_failed",
                     error = &error as &(dyn std::error::Error + 'static),
                     decision = "retain_verification_evidence");
                self.warnings.push(error);
                self.verification == Some(Verification::Fenced)
            }
        };
        self.verification = Some(if fenced {
            Verification::Fenced
        } else {
            Verification::Pending
        });
        self.outcome = Some(if fenced {
            AuthOutcome::RepairRequired
        } else {
            AuthOutcome::VerificationPending
        });
    }
    pub(super) fn bind(&mut self, state: &State, binding: &Binding) -> Result<()> {
        self.state_directory = Some(state.root.clone());
        self.generation = Some(binding.id);
        self.retired_controller = state.retired_controller()?;
        Ok(())
    }
    pub(super) fn finish_attempt(&mut self, operation: Operation) {
        if let Err(error) = operation.finish() {
            tracing::warn!(name: "auth.attempt_cleanup_failed",
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "preserve_report");
            self.warnings.push(error);
        }
    }
    pub(super) fn repaired(&mut self, activated: bool, authenticated: bool) {
        self.outcome = Some(if activated {
            AuthOutcome::GenerationReplaced
        } else if authenticated {
            AuthOutcome::RepairSuperseded
        } else {
            AuthOutcome::RepairRetired
        });
        if !activated {
            self.retired_verification(true);
        }
    }
    pub(super) fn retired_verification(&mut self, repair: bool) {
        if self.verification.is_some() {
            let message = if repair {
                concat!(
                    "This repair candidate retains unresolved verification evidence. Use the ",
                    "reported state_directory with auth repair PROFILE --reauthenticate to ",
                    "try a new candidate; omit --new-state-dir. The retired generation can be ",
                    "pruned after ownership is proven released.",
                )
            } else {
                concat!(
                    "This superseded login retains unresolved verification evidence. Check ",
                    "auth status for the current profile; auth recover cannot verify the ",
                    "reported retired generation. It can be pruned after ownership is proven ",
                    "released.",
                )
            };
            let mut error = Error::recovery_required(message);
            if let Some(original) = self.diagnostic.take() {
                error = error.with_source(original);
            }
            self.fail(error);
        }
    }
}
