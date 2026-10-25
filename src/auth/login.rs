//! Interactive login and credential verification.
use super::{
    Auth, AuthOperation, AuthOutcome, Binding, Error, IsTerminal, LoginOutcome, Method, Operation,
    Profile, Provider, Report, Result, Status, process, state, verify_pending,
};
use crate::state::OperationKind;
use std::{io, ops::ControlFlow, path::Path};

impl Auth {
    pub(crate) fn login(&self, name: &str) -> Result<LoginOutcome> {
        let profile = self.config.profile(name)?;
        if self.settings.browser && profile.provider != Provider::Codex {
            return Err(Error::invalid_option(
                "The --browser option applies only to Codex profiles.",
            ));
        }
        let mut report = Report::new(
            name,
            profile,
            crate::observability::optional(self.binary(profile), "login_binary_metadata"),
        );
        report.operation = AuthOperation::Login;
        if profile.method == Method::ApiKey {
            return self.register_key(name, profile, report);
        }
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err(Error::interactive_terminal_required(
                "Subscription login requires a terminal for stdin and stdout.",
            ));
        }
        tracing::info!(name: "auth.login", provider = profile.provider.as_str(), phase = "started");
        let outcome = self.login_subscription(name, profile, report);
        tracing::info!(name: "auth.login",
             provider = profile.provider.as_str(),
             phase = "finished",
             success = outcome.as_ref().is_ok_and(|outcome| outcome.success));
        outcome
    }
    fn register_key(
        &self,
        name: &str,
        profile: &Profile,
        mut report: Report,
    ) -> Result<LoginOutcome> {
        let binding = self.state.register(name, profile)?;
        report.bind(&self.state, &binding)?;
        // Registration grants no native authentication claim or writer bypass.
        self.state
            .operation(&binding, OperationKind::Status)?
            .finish()?;
        report = super::api_key_status(profile, report, self.binary(profile));
        report.outcome = Some(AuthOutcome::Registered);
        Ok(LoginOutcome {
            success: report.state == Status::PresentUnverified,
            report,
        })
    }
    pub(super) fn login_subscription(
        &self,
        name: &str,
        profile: &Profile,
        mut report: Report,
    ) -> Result<LoginOutcome> {
        let binary = self.report_binary(profile, &mut report)?;
        let binding = self.state.register(name, profile)?;
        let session = ProfileSession::begin(self, profile, &binding, &mut report)?;
        let Some(session) = session.verify_initial_login() else {
            return Ok(LoginOutcome {
                report,
                success: false,
            });
        };
        let session = match session.select_login(name)? {
            ControlFlow::Continue(session) => session,
            ControlFlow::Break(success) => return Ok(LoginOutcome { report, success }),
        };
        let success = session.run(&binary)?;
        Ok(self.finish_login_report(name, profile, &binding, report, success))
    }
    pub(super) fn finish_login_report(
        &self,
        name: &str,
        profile: &Profile,
        binding: &Binding,
        mut report: Report,
        success: bool,
    ) -> LoginOutcome {
        let active = match self.state.lookup(name, profile) {
            Ok(current) => current.is_some_and(|current| current.id == binding.id),
            Err(error) => {
                if report.verification.is_none()
                    && matches!(self.state.pending_credential_attempt(binding.id), Ok(None))
                {
                    report.fail(error);
                    report.outcome = Some(AuthOutcome::LoginSelectionUnknown);
                } else {
                    report.verification_failed(&self.state, binding, error);
                }
                return LoginOutcome {
                    report,
                    success: false,
                };
            }
        };
        if !active {
            report.outcome = Some(AuthOutcome::LoginSuperseded);
            report.retired_verification(false);
        }
        LoginOutcome {
            report,
            success: success && active,
        }
    }
}

pub(super) struct ProfileSession<'a> {
    pub(super) auth: &'a Auth,
    pub(super) profile: &'a Profile,
    pub(super) binding: &'a Binding,
    pub(super) operation: Operation,
    pub(super) report: &'a mut Report,
}

impl<'a> ProfileSession<'a> {
    fn verify_initial_login(self) -> Option<Self> {
        let verified = self.operation.has_pending().and_then(|pending| {
            if pending {
                verify_pending(&self.operation, self.report)
            } else {
                Ok(())
            }
        });
        if let Err(error) = verified {
            self.report
                .verification_failed(&self.auth.state, self.binding, error);
            self.report.finish_attempt(self.operation);
            return None;
        }
        Some(self)
    }
    fn select_login(self, name: &str) -> Result<ControlFlow<bool, Self>> {
        if self.report.state == Status::Authenticated && !self.auth.settings.reauthenticate {
            let active = self
                .auth
                .state
                .lookup(name, self.profile)?
                .is_some_and(|b| b.id == self.binding.id);
            self.report.outcome = Some(if active {
                AuthOutcome::AlreadyAuthenticated
            } else {
                AuthOutcome::LoginSuperseded
            });
            self.auth
                .state
                .record_account(self.binding, self.report.account_identity.clone())?;
            self.operation.finish()?;
            return Ok(ControlFlow::Break(active));
        }
        let can_login = matches!(
            self.report.state,
            Status::Missing | Status::ReauthenticationRequired
        ) || (self.report.state == Status::Authenticated
            && self.auth.settings.reauthenticate);
        if !can_login {
            self.report.outcome = Some(if self.auth.settings.cancellation.count() > 0 {
                AuthOutcome::Cancelled
            } else {
                AuthOutcome::LoginRefused
            });
            self.operation.finish()?;
            return Ok(ControlFlow::Break(false));
        }
        if self.report.state == Status::Authenticated {
            self.auth
                .state
                .record_account(self.binding, self.report.account_identity.clone())?;
        }
        Ok(ControlFlow::Continue(self))
    }
    pub(super) fn begin(
        auth: &'a Auth,
        profile: &'a Profile,
        binding: &'a Binding,
        report: &'a mut Report,
    ) -> Result<Self> {
        report.bind(&auth.state, binding)?;
        let mut operation = auth.state.operation(binding, OperationKind::Login)?;
        auth.probe(
            profile,
            binding,
            &mut operation,
            report,
            auth.settings.cancellation.clone(),
        );
        Ok(Self {
            auth,
            profile,
            binding,
            operation,
            report,
        })
    }

    pub(super) fn run(mut self, binary: &Path) -> Result<bool> {
        let dir = self.binding.directory.clone();
        let mut cmd = process::command(
            binary,
            self.profile.provider,
            &dir.join("provider"),
            &dir.join("work"),
        );
        let _browser_lock = match self.browser_lock() {
            Ok(lock) => lock,
            Err(error) => {
                self.operation.finish()?;
                return Err(if matches!(error, Error::ProfileBusy { .. }) {
                    Error::codex_browser_login_busy(
                        "Another Codex browser login owns this state directory's callback lock. \
                        Retry after it finishes.",
                    )
                    .with_source(error)
                } else {
                    error
                });
            }
        };
        match self.profile.provider {
            Provider::Codex => {
                cmd.arg("login");
                if !self.auth.settings.browser {
                    cmd.arg("--device-auth");
                }
            }
            Provider::Claude => {
                cmd.args(["auth", "login", "--claudeai"]);
            }
        }
        let outcome = process::interactive(
            &self.auth.settings.launcher,
            &mut cmd,
            &mut self.operation,
            self.auth.settings.login_timeout,
            &self.auth.settings.cancellation,
        );
        match outcome {
            Err(error) => {
                if matches!(error, Error::SpawnFailed { .. }) {
                    self.operation.finish()?;
                }
                if matches!(
                    self.auth.state.pending_credential_attempt(self.binding.id),
                    Ok(None)
                ) {
                    self.report.fail(error);
                } else {
                    self.report
                        .verification_failed(&self.auth.state, self.binding, error);
                }
                Ok(false)
            }
            Ok(exit) => self.finish_login(exit),
        }
    }
    fn browser_lock(&self) -> Result<Option<state::Lock>> {
        if self.profile.provider == Provider::Codex && self.auth.settings.browser {
            state::Lock::acquire(&self.auth.state.root.join("codex-browser.lock")).map(Some)
        } else {
            Ok(None)
        }
    }
    // Consume the completed login outcome at the operation-finalization boundary.
    fn finish_login(mut self, exit: process::LoginExit) -> Result<bool> {
        let cancelled = self.auth.settings.cancellation.count() > 0 && !exit.status.success();
        // A fresh OFD proves no descendant retained the writer's credential lock.
        // Keep the writer receipt unchanged; failure leaves it recoverable later.
        drop(self.operation);
        self.operation = match self
            .auth
            .state
            .operation(self.binding, OperationKind::Recover)
        {
            Ok(operation) => operation,
            Err(error) => {
                self.report.verification_failed(
                    &self.auth.state,
                    self.binding,
                    Error::recovery_required(concat!(
                        "Login may have saved credentials, but exclusive verification is \
                            pending. ",
                        "Retry auth recover after current credential users exit.",
                    ))
                    .with_source(error),
                );
                return Ok(false);
            }
        };
        self.auth.probe(
            self.profile,
            self.binding,
            &mut self.operation,
            self.report,
            self.auth.settings.cancellation.checkpoint(),
        );
        let before = match self.operation.has_pending() {
            Ok(pending) => pending,
            Err(error) => {
                self.report
                    .verification_failed(&self.auth.state, self.binding, error);
                return Ok(false);
            }
        };
        let verified = if before {
            verify_pending(&self.operation, self.report)
        } else {
            // Another exclusive recover may have verified the original writer
            // while we released its OFD. Compare with that saved expectation.
            self.auth
                .state
                .check_account(self.binding, self.report.account_identity.as_deref())
        };
        self.conclude_login(exit, cancelled, verified)
    }

    fn conclude_login(
        self,
        exit: process::LoginExit,
        cancelled: bool,
        verified: Result<()>,
    ) -> Result<bool> {
        let success = !exit.timed_out
            && !cancelled
            && exit.status.success()
            && self.report.state == Status::Authenticated;
        let pending = match self.operation.has_pending() {
            Ok(pending) => pending,
            Err(error) => {
                self.report
                    .verification_failed(&self.auth.state, self.binding, error);
                return Ok(false);
            }
        };
        if pending {
            self.report.verification_failed(
                &self.auth.state,
                self.binding,
                verified
                    .err()
                    .unwrap_or_else(|| Error::recovery_required(LOGIN_VERIFICATION_PENDING)),
            );
            // Unresolved writer receipts are deliberately retained; do not turn
            // this report back into a generic error at the finalization boundary.
            if let Err(error) = self.operation.finish() {
                tracing::warn!(name: "auth.finalization_failed",
                     code = error.code(),
                     error = &error as &(dyn std::error::Error + 'static),
                     decision = "preserve_verification_failure");
            }
            return Ok(false);
        }
        if let Err(error) = verified {
            self.report.fail(error);
            self.report.outcome = Some(AuthOutcome::VerificationFailed);
            self.operation.finish()?;
            return Ok(false);
        }
        self.report.outcome = Some(if exit.timed_out {
            AuthOutcome::Timeout
        } else if cancelled {
            AuthOutcome::Cancelled
        } else if success {
            AuthOutcome::Authenticated
        } else {
            AuthOutcome::LoginFailed
        });
        if exit.timed_out {
            self.report.diagnostic = Some(Error::timeout(
                "Provider login exceeded the configured login timeout.",
            ));
        } else if cancelled {
            self.report.diagnostic = Some(Error::cancelled(
                "Login was cancelled; saved credentials were retained.",
            ));
        } else if !success && self.report.diagnostic.is_none() {
            self.report.diagnostic = Some(Error::login_failed(LOGIN_METHOD_FAILED));
        }
        self.operation.finish()?;
        Ok(success)
    }
}

const LOGIN_VERIFICATION_PENDING: &str = concat!(
    "Login verification is pending. Retry auth recover; credentials and ",
    "evidence were retained.",
);

const LOGIN_METHOD_FAILED: &str = concat!(
    "Provider login did not complete with the selected method. For Codex ",
    "device-login failures, retry with --browser.",
);
