//! Isolated credential-generation repair and activation.
use super::{
    Auth, AuthOperation, AuthOutcome, Binding, Error, IsTerminal, LoginOutcome, Method,
    PermissionsExt, Profile, ProfileSession, Provider, Report, Result, State, Status,
    parse_version, state,
};
use crate::{os, process};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

impl Auth {
    pub(crate) fn repair(
        &mut self,
        name: &str,
        destination: Option<&Path>,
    ) -> Result<LoginOutcome> {
        if destination.is_none() {
            self.state.check_format()?;
        }
        let profile = self.config.profile(name)?;
        self.validate_repair(profile)?;
        let destination = self.repair_destination(name, profile, destination)?;
        let binary = self.binary(profile)?;
        let mut report = Report::new(name, profile, Some(binary.clone()));
        report.operation = AuthOperation::Repair;
        if profile.provider == Provider::Claude
            && let Err(error) = self.repair_version(&binary, destination.as_deref(), &mut report)
        {
            report.outcome = Some(AuthOutcome::RepairRefused);
            report.fail(error);
            return Ok(LoginOutcome {
                report,
                success: false,
            });
        }
        let prepared = prepare_repair(&mut self.state, name, profile, destination, &mut report);
        let repair = match prepared {
            Ok(repair) => repair,
            Err(error) => {
                report.fail(error);
                report.outcome = Some(AuthOutcome::RepairRefused);
                return Ok(LoginOutcome {
                    report,
                    success: false,
                });
            }
        };
        let success = match self.attempt_repair(profile, &repair.binding, &binary, &mut report) {
            Ok(success) => success,
            Err(error) => {
                report.fail(error);
                false
            }
        };
        Ok(self.finish_repair_report(&repair, LoginOutcome { report, success }))
    }
    fn repair_destination(
        &self,
        name: &str,
        profile: &Profile,
        destination: Option<&Path>,
    ) -> Result<Option<PathBuf>> {
        let destination = destination
            .map(|path| self.state.replacement_destination(path))
            .transpose()?;
        if destination.is_none() {
            let binding = self
                .state
                .lookup(name, profile)?
                .ok_or(Error::login_required(state::REGISTER_PROFILE_FIRST))?;
            if !binding.ready {
                return Err(Error::registration_incomplete(
                    state::REGISTRATION_INCOMPLETE,
                ));
            }
        }
        if destination.is_some() {
            self.state.replacement_metadata(profile.provider)?;
        }
        Ok(destination)
    }
    pub(super) fn attempt_repair(
        &self,
        profile: &Profile,
        binding: &Binding,
        binary: &Path,
        report: &mut Report,
    ) -> Result<bool> {
        let session = ProfileSession::begin(self, profile, binding, report)?;
        if session.report.state != Status::Missing {
            session.operation.finish()?;
            session.report.outcome = Some(AuthOutcome::RepairRefused);
            session.report.diagnostic = Some(Error::policy_conflict(
                "Fresh repair namespace was not verified unauthenticated.",
            ));
            return Ok(false);
        }
        session.run(binary)
    }
    pub(super) fn finish_repair_report(
        &self,
        repair: &state::Repair,
        mut outcome: LoginOutcome,
    ) -> LoginOutcome {
        let authenticated = outcome.success;
        let account = outcome.report.account_identity.clone();
        let activated = match self.state.finish_repair(repair, authenticated, account) {
            Ok(activated) => activated,
            Err(error) => {
                let retired = self.state.retire_repair(repair);
                outcome.success = false;
                if let Some(error) = outcome.report.diagnostic.take() {
                    outcome.report.warnings.push(error);
                }
                outcome.report.fail(error);
                outcome.report.outcome = Some(if retired.is_ok() {
                    AuthOutcome::RepairRetired
                } else {
                    AuthOutcome::RepairFinalizationFailed
                });
                if retired.is_ok() {
                    outcome.report.retired_verification(true);
                }
                if let Err(error) = retired {
                    outcome.report.warnings.push(error);
                }
                return outcome;
            }
        };
        outcome.success = activated;
        outcome.report.repaired(activated, authenticated);
        outcome
    }
    pub(super) fn validate_repair(&self, profile: &Profile) -> Result<()> {
        if profile.method != Method::Subscription {
            return Err(Error::invalid_option(
                "Profile repair requires a subscription profile and --reauthenticate.",
            ));
        }
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err(Error::interactive_terminal_required(
                "Subscription repair requires a terminal for stdin and stdout.",
            ));
        }
        if self.settings.browser && profile.provider != Provider::Codex {
            return Err(Error::invalid_option(
                "--browser is only supported for Codex.",
            ));
        }
        Ok(())
    }
    pub(super) fn repair_version(
        &self,
        binary: &Path,
        destination: Option<&Path>,
        report: &mut Report,
    ) -> Result<()> {
        let temporary = destination
            .map(|path| -> Result<_> {
                let parent = path
                    .parent()
                    .ok_or(Error::invalid_path("Missing replacement parent."))?;
                // Keep before native execution: TempDir must never auto-delete uncertain evidence.
                let root = tempfile::Builder::new()
                    .prefix(".hive-probe-")
                    .permissions(fs::Permissions::from_mode(os::PRIVATE_DIRECTORY_MODE))
                    .tempdir_in(parent)
                    .map_err(Error::state_io_from)?
                    .keep();
                report.metadata_probe_directory = Some(root.clone());
                let state = State::new(root)?;
                let identity = state.metadata_root_identity()?;
                if let Err(error) = state::sync_directory(parent) {
                    let cleanup = state.remove_empty_metadata_root(&identity);
                    if cleanup.removed {
                        report.metadata_probe_directory = None;
                    }
                    if let Some(warning) = cleanup.warning {
                        report.warnings.push(warning);
                    }
                    return Err(Error::state_io_from(error));
                }
                Ok((state, identity))
            })
            .transpose()?;
        let state = temporary.as_ref().map_or(&self.state, |(state, _)| state);
        let result = self.probe_version(binary, state);
        if let Some((_, identity)) = &temporary {
            let cleanup = state.remove_empty_metadata_root(identity);
            if cleanup.removed {
                report.metadata_probe_directory = None;
            }
            if let Some(warning) = cleanup.warning {
                report.warnings.push(warning);
            }
        }
        result
    }
    pub(super) fn probe_version(&self, binary: &Path, state: &State) -> Result<()> {
        let mut operation = state.metadata_probe()?;
        let result = self.run_version_probe(binary, &mut operation);
        let finished = operation.finish();
        state.sweep_metadata_probes();
        crate::observability::settle(result, finished, "auth_version_probe")
    }
    fn run_version_probe(&self, binary: &Path, operation: &mut super::Operation) -> Result<()> {
        let mut command = operation.version_command(binary, Provider::Claude)?;
        process::version_probe(
            &self.settings.launcher,
            &mut command,
            operation,
            self.settings.probe_timeout,
            self.settings.cancellation.clone(),
            None,
            |output, operation| {
                if !output.status.success() {
                    return Err(Error::provider_failed(super::VERSION_PROBE_FAILED));
                }
                operation.verify_probe_ownership().map_err(|error| {
                    Error::provider_failed(concat!(
                        "Version probe ownership could not be confirmed; its isolated evidence ",
                        "may be retained. Retry with the verified provider executable.",
                    ))
                    .with_source(error)
                })?;
                let version = parse_version(Provider::Claude, &output.stdout)?;
                crate::provider_support::classify(Provider::Claude, &version)?.admit()?;
                Ok(())
            },
        )
    }
}

fn prepare_repair(
    state: &mut State,
    name: &str,
    profile: &Profile,
    destination: Option<PathBuf>,
    report: &mut Report,
) -> Result<state::Repair> {
    if let Some(destination) = destination {
        report.state_directory = Some(destination.clone());
        let replacement = state.replacement_root(&destination, profile.provider)?;
        replacement.register(name, profile)?;
        *state = replacement;
    }
    state.begin_repair(name, profile)
}
