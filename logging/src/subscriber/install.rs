use super::Backend;
use std::{error::Error, fmt};
use tracing_log::AsLog;
use tracing_subscriber::prelude::*;

/// Global registration is one-shot. It cannot roll back pre-existing globals.
#[derive(Debug)]
pub enum InstallError {
    /// The pre-existing log facade remains unchanged; our subscriber was not set.
    BridgeConflict(log::SetLoggerError),
    /// The bridge was installed and forwards to the pre-existing subscriber.
    /// Our backend was not installed, so its Guard can safely be finished.
    SubscriberConflictWithBridgeInstalled(tracing::subscriber::SetGlobalDefaultError),
}
impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::BridgeConflict(_) => {
                "Log bridge is already installed; tracing backend was not installed."
            }
            Self::SubscriberConflictWithBridgeInstalled(_) => {
                "Tracing subscriber is already installed; the new log bridge forwards to \
                that subscriber. The new backend was not installed."
            }
        })
    }
}
impl Error for InstallError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::BridgeConflict(error) => Some(error),
            Self::SubscriberConflictWithBridgeInstalled(error) => Some(error),
        }
    }
}
impl Backend {
    /// Route synchronous work to this backend without changing process globals.
    /// The caller retains its guard until work and bounded shutdown finish.
    pub fn scope<T>(self, work: impl FnOnce() -> T) -> T {
        tracing::subscriber::with_default(tracing_subscriber::registry().with(self), work)
    }

    /// Install at process startup. Prefer `registry().with(backend)` for scoped
    /// subscribers/tests. A global conflict is a startup error, never retried.
    pub fn install_global(self) -> Result<(), InstallError> {
        tracing_log::LogTracer::builder()
            .with_max_level(self.logger.shared.config.library_max().as_log())
            .init()
            .map_err(InstallError::BridgeConflict)?;
        tracing::subscriber::set_global_default(tracing_subscriber::registry().with(self))
            .map_err(InstallError::SubscriberConflictWithBridgeInstalled)
    }
}
