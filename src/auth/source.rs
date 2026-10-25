//! Presence observation runs in the process whose environment owns future launches.
use super::{AuthOperation, AuthOutcome, Report};
use crate::{
    config::Profile,
    error::Result,
    state::{OperationKind, State},
};
use std::path::PathBuf;

pub(crate) struct Input {
    pub name: String,
    pub profile: Profile,
    pub state: State,
    pub binary: Result<PathBuf>,
    pub register: bool,
}
impl Input {
    pub(crate) fn resolve(self) -> Result<Report> {
        let mut report = Report::new(
            &self.name,
            &self.profile,
            self.binary.as_ref().ok().cloned(),
        );
        let binding = if self.register {
            Some(self.state.register(&self.name, &self.profile)?)
        } else {
            self.state.lookup(&self.name, &self.profile)?
        };
        if let Some(binding) = binding {
            report.bind(&self.state, &binding)?;
            self.state
                .operation(&binding, OperationKind::Status)?
                .finish()?;
        }
        report = super::api_key_status(&self.profile, report, self.binary);
        report.environment_source = "daemon";
        if self.register {
            report.operation = AuthOperation::Login;
            report.outcome = Some(AuthOutcome::Registered);
        }
        // Presence proves only local readability; never remote validity.
        Ok(report)
    }
}
