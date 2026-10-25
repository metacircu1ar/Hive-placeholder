//! Owned provider metadata and account probes.
// The short-lived status probe sends exactly these two sequential RPC requests.
use super::{
    Auth, Binding, Cancellation, Captured, Error, Operation, Profile, Provider, Report, Result,
    parse_claude, parse_codex, parse_version, process,
};
use std::process::Command;
const INITIALIZE_REQUEST_ID: u64 = 1;
const ACCOUNT_REQUEST_ID: u64 = 2;
mod wire;

impl Auth {
    pub(super) fn probe(
        &self,
        profile: &Profile,
        binding: &Binding,
        operation: &mut Operation,
        report: &mut Report,
        cancellation: Cancellation,
    ) {
        report.diagnostic = None;
        report.warnings.clear();
        report.credential_present = None;
        report.observed_method = None;
        report.provider_version = None;
        report.provider_support = None;
        report.account_identity = None;
        let result = self.probe_owned(profile, binding, operation, report, &cancellation);
        if let Err(error) = result {
            report.fail(error);
        }
    }
    fn probe_owned(
        &self,
        profile: &Profile,
        binding: &Binding,
        operation: &mut Operation,
        report: &mut Report,
        cancellation: &Cancellation,
    ) -> Result<()> {
        let binary = self.report_binary(profile, report)?;
        let identity = crate::runtime::provider_binary_identity(&binary)?;
        operation.verify_binary(identity.clone())?;
        let dir = binding.directory.clone();
        let build = || {
            process::command(
                &binary,
                profile.provider,
                &dir.join("provider"),
                &dir.join("work"),
            )
        };
        // Version output is allowed only in its exact known non-secret format.
        let mut version_cmd = build();
        version_cmd.arg("--version");
        let version = process::version_probe(
            &self.settings.launcher,
            &mut version_cmd,
            operation,
            self.settings.probe_timeout,
            cancellation.clone(),
            None,
            |output, _| {
                if !output.status.success() {
                    return Err(Error::provider_failed(super::VERSION_PROBE_FAILED));
                }
                // Unrecognized metadata is a warning for account probes.
                Ok(parse_version(profile.provider, &output.stdout)
                    .inspect_err(|error| {
                        tracing::debug!(name: "auth.version_unrecognized",
                         error = error as &(dyn std::error::Error + 'static),
                         decision = "check_account_separately");
                    })
                    .ok())
            },
        )?;
        operation.verify_binary(identity)?;
        report.provider_version = version;
        if let Some(version) = &report.provider_version {
            report.provider_support = Some(crate::provider_support::classify(
                profile.provider,
                version,
            )?);
            if let Some(observation) = &report.provider_support {
                observation.admit()?;
            }
        }
        if report.provider_version.is_none() {
            report.warnings.push(Error::version_unrecognized(
                "Provider version text was not recognized; authentication was checked \
                    separately.",
            ));
        }
        self.probe_account(profile.provider, build, operation, report, cancellation)
    }
    pub(super) fn probe_account(
        &self,
        provider: Provider,
        build: impl Fn() -> Command,
        operation: &mut Operation,
        report: &mut Report,
        cancellation: &Cancellation,
    ) -> Result<()> {
        match provider {
            Provider::Codex => self.probe_codex(build, operation, report, cancellation),
            Provider::Claude => {
                let mut cmd = build();
                cmd.args(["auth", "status", "--json"]);
                let mut child = Captured::start(
                    &self.settings.launcher,
                    &mut cmd,
                    operation,
                    self.settings.probe_timeout,
                    cancellation.clone(),
                )?;
                let process::CapturedOutput {
                    status,
                    stdout: bytes,
                } = child.output()?;
                parse_claude(status.code(), &bytes, report)
            }
        }
    }
    fn probe_codex(
        &self,
        build: impl Fn() -> Command,
        operation: &mut Operation,
        report: &mut Report,
        cancellation: &Cancellation,
    ) -> Result<()> {
        let mut cmd = build();
        cmd.arg("app-server");
        let mut child = Captured::start(
            &self.settings.launcher,
            &mut cmd,
            operation,
            self.settings.probe_timeout,
            cancellation.clone(),
        )?;
        child.send(&wire::initialize())?;
        let hello = child.reply(INITIALIZE_REQUEST_ID)?;
        if hello.get("result").is_none() {
            return Err(Error::provider_protocol_error(
                "Provider initialization failed.",
            ));
        }
        child.send(&wire::initialized())?;
        child.send(&wire::account_read())?;
        let result = child.reply(ACCOUNT_REQUEST_ID)?;
        let parsed = parse_codex(&result, report);
        if !child.shutdown()? {
            report.warnings.push(Error::provider_shutdown_forced(
                "Provider shutdown exceeded its grace period; the captured account result \
                    was retained.",
            ));
        }
        parsed
    }
}
