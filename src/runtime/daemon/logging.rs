//! One process owner spans preparation, Tokio teardown, and bounded diagnostic drain.
use crate::{
    error::{Error, Result},
    state::{self, Lock},
};
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::level_filters::LevelFilter;

const FINISH_BUDGET: Duration = Duration::from_secs(1);

pub(super) fn configuration(settings: &crate::config::Logging) -> Result<hive_log::Config> {
    let mut config = settings.backend()?;
    let level = match std::env::var_os("HIVE_LOG") {
        None => config.level,
        Some(value) => match value.to_str() {
            Some("off") => LevelFilter::OFF,
            Some("error") => LevelFilter::ERROR,
            Some("warn") => LevelFilter::WARN,
            Some("info") => LevelFilter::INFO,
            Some("debug") => LevelFilter::DEBUG,
            Some("trace") => LevelFilter::TRACE,
            _ => {
                return Err(Error::config_invalid(
                    "HIVE_LOG must be off, error, warn, info, debug or trace.",
                ));
            }
        },
    };
    config.level = level;
    Ok(config)
}

pub(super) struct ProcessLog {
    guard: hive_log::Guard,
    started: Instant,
}
impl ProcessLog {
    pub fn start(runtime: &Path, config: hive_log::Config, owner: Arc<Lock>) -> Result<Self> {
        let directory = runtime.join("logs");
        state::directory(&directory)?;
        let (backend, guard) =
            hive_log::start_owned(&directory.join("hive-daemon.jsonl"), config, owner)
                .map_err(Error::state_io_from)?;
        if let Err(cause) = backend.clone().install_global() {
            // No backend was installed; stop its unused worker without replacing globals.
            guard.finish(FINISH_BUDGET);
            return Err(Error::state_io_from(std::io::Error::other(cause)));
        }
        backend.install_panic_hook();
        let features = if cfg!(feature = "comb") {
            "comb"
        } else {
            "core"
        };
        tracing::info!(name: "daemon.starting",
             pid = std::process::id(),
             version = env!("CARGO_PKG_VERSION"),
             features = features);
        tracing::info!(name: "daemon.lock_acquired", success = true);
        Ok(Self {
            guard,
            started: Instant::now(),
        })
    }
    pub fn finish(self, result: Result<()>) -> Result<()> {
        crate::observability::finish();
        if let Err(error) = &result {
            tracing::error!(name: "daemon.failed",
                 target: "hive.fatal",
                 code = error.code(),
                 sqlite_code = super::super::store::sqlite_code(error),
                 error = error as &(dyn std::error::Error + 'static),
                 decision = "shutdown");
        }
        tracing::info!(name: "daemon.stopped",
             success = result.is_ok(),
             elapsed_ms = self.started.elapsed().as_millis());
        let finished = self.guard.finish(FINISH_BUDGET);
        if result.is_ok() {
            // Name the writer's state, so an unsuccessful exit is attributable.
            let incomplete = match finished.state {
                hive_log::Shutdown::Drained => None,
                hive_log::Shutdown::Unconfirmed => {
                    Some("The diagnostic log did not finish draining within its shutdown budget.")
                }
                hive_log::Shutdown::WriteFailed => {
                    Some("The diagnostic log could not be written at shutdown.")
                }
                hive_log::Shutdown::Panicked => {
                    Some("The diagnostic log writer stopped unexpectedly at shutdown.")
                }
            };
            if let Some(message) = incomplete {
                return Err(Error::state_io(message));
            }
        }
        result
    }
}

/// Report every boundary failure, while returning the first primary error unchanged.
pub(super) fn results<const N: usize>(outcomes: [(&'static str, Result<()>); N]) -> Result<()> {
    let mut primary = Ok(());
    for (phase, outcome) in outcomes {
        if let Err(error) = outcome {
            tracing::error!(name: "daemon.service_failed",
                 phase = phase,
                 code = error.code(),
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "shutdown");
            if primary.is_ok() {
                primary = Err(error);
            }
        }
    }
    primary
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Test error assertions must fail immediately.
mod tests {
    use super::*;
    #[test]
    fn cleanup_never_replaces_the_primary_failure() {
        let primary = Error::state_io_from(std::io::Error::from_raw_os_error(28));
        let secondary = Error::daemon_stopping("Cleanup failed.");
        let error =
            results([("execution", Err(primary)), ("cleanup", Err(secondary))]).unwrap_err();
        assert_eq!(error.io_raw_os_error(), Some(28));
        assert_eq!(
            results([
                ("execution", Ok(())),
                ("cleanup", Err(Error::daemon_stopping("Cleanup failed.")))
            ])
            .unwrap_err()
            .code(),
            "daemon_stopping"
        );
    }
}
