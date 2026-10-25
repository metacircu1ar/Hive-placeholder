//! Authentication profile operations and native-provider selection.
use crate::{
    config::{Config, Method, Profile, Provider},
    error::{Error, Result},
    launch::LauncherCache,
    process::{self, Cancellation, Captured},
    runtime::api::value as project,
    state::{self, Binding, Operation, OperationKind, State},
};
use login::ProfileSession;
use parse::{parse_claude, parse_codex, verify_pending};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::{
    ffi::OsString, io::IsTerminal, os::unix::fs::PermissionsExt, path::PathBuf, sync::Arc,
    time::Duration,
};
use uuid::Uuid;
const VERSION_PROBE_FAILED: &str = "Provider version probe failed.";

mod credential;
mod keys;
pub(crate) use keys::Keys;
mod key;
pub(crate) use credential::{ApiLogin, Credential};
mod login;
mod parse;
mod probe;
mod recover;
mod repair;
mod report;
pub(crate) mod source;

pub(crate) use parse::parse_version;
pub(crate) use report::{AuthOperation, AuthOutcome, ObservedMethod, Report, Status, Verification};

pub(crate) struct Settings {
    pub(crate) launcher: Arc<LauncherCache>,
    pub(crate) codex: OsString,
    pub(crate) claude: OsString,
    pub(crate) probe_timeout: Duration,
    pub(crate) login_timeout: Duration,
    pub(crate) browser: bool,
    pub(crate) reauthenticate: bool,
    pub(crate) cancellation: Cancellation,
}

pub(crate) struct LoginOutcome {
    pub(crate) report: Report,
    pub(crate) success: bool,
}
pub(crate) struct RecoveryOutcome {
    pub(crate) report: Value,
    pub(crate) success: bool,
}

pub(crate) struct Auth {
    pub(crate) config: Config,
    pub(crate) state: State,
    pub(crate) settings: Settings,
}
#[derive(Serialize)]
struct Profiles<'a> {
    operation: AuthOperation,
    profiles: Vec<ProfileSummary<'a>>,
}
#[derive(Serialize)]
struct ProfileSummary<'a> {
    name: &'a str,
    provider: Provider,
    method: Method,
    api_key_env: Option<&'a str>,
    api_key_file: Option<&'a std::path::Path>,
    profile_id: Option<Uuid>,
    compatibility: crate::provider_support::Registry,
}
impl Auth {
    fn binary(&self, profile: &Profile) -> Result<PathBuf> {
        process::executable(match profile.provider {
            Provider::Codex => &self.settings.codex,
            Provider::Claude => &self.settings.claude,
        })?
        .canonicalize()
        .map_err(Error::state_io_from)
    }
    fn report_binary(&self, profile: &Profile, report: &mut Report) -> Result<PathBuf> {
        let binary = report
            .provider_executable
            .take()
            .map_or_else(|| self.binary(profile), Ok)?;
        report.provider_executable = Some(binary.clone());
        Ok(binary)
    }
    pub(crate) fn status(&self, name: &str) -> Result<Report> {
        let profile = self.config.profile(name)?;
        let binding = self.state.lookup(name, profile)?;
        let binary = self.binary(profile);
        let mut report = Report::new(name, profile, binary.as_ref().ok().cloned());
        if profile.method == Method::ApiKey {
            if let Some(binding) = binding {
                report.bind(&self.state, &binding)?;
            }
            return Ok(api_key_status(profile, report, binary));
        }
        let Some(binding) = binding else {
            report.credential_present = Some(false);
            if let Err(error) = binary {
                report.diagnostic = Some(error);
            }
            return Ok(report);
        };
        report.bind(&self.state, &binding)?;
        if !binding.ready {
            report.credential_present = Some(false);
            report.diagnostic = Some(Error::registration_incomplete(
                "Retry auth login to finish registering this profile.",
            ));
            return Ok(report);
        }
        let mut operation = match self.state.operation(&binding, OperationKind::Status) {
            Ok(operation) => operation,
            Err(error) => {
                report.fail(error);
                return Ok(report);
            }
        };
        self.probe(
            profile,
            &binding,
            &mut operation,
            &mut report,
            self.settings.cancellation.clone(),
        );
        if report.state == Status::Authenticated {
            self.state
                .check_account(&binding, report.account_identity.as_deref())?;
        }
        operation.finish()?;
        Ok(report)
    }
    // Own the cancellation checkpoint until probing and report updates finish.
}

fn api_key_status(profile: &Profile, mut report: Report, binary: Result<PathBuf>) -> Report {
    match key::resolve(profile) {
        Ok(_) => {
            report.credential_present = Some(true);
            report.state = Status::PresentUnverified;
        }
        Err(error @ Error::AuthenticationRequired { .. }) => {
            report.credential_present = Some(false);
            report.state = Status::Missing;
            report.diagnostic = Some(error);
        }
        Err(error) => report.fail(error),
    }
    if let Err(error) = binary {
        report.warnings.push(error);
    }
    report
}

pub(crate) fn list_profiles(config: &Config, state: &State) -> Result<Value> {
    let profiles: Result<Vec<_>> = config
        .auth_profiles
        .iter()
        .map(|(name, profile)| {
            let binding = state.lookup(name, profile)?;
            Ok(ProfileSummary {
                name,
                provider: profile.provider,
                method: profile.method,
                api_key_env: profile.api_key_env.as_deref(),
                api_key_file: profile.api_key_file.as_deref(),
                profile_id: binding.map(|b| b.id),
                compatibility: crate::provider_support::registry(profile.provider),
            })
        })
        .collect();
    Ok(project(Profiles {
        operation: AuthOperation::List,
        profiles: profiles?,
    }))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) mod cli_tests;
