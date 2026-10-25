//! Bounded standalone diagnostics begin only after managed environment loading.
use comb::{Error, Result};
use std::time::Duration;
use tracing::level_filters::LevelFilter;

const DRAIN: Duration = Duration::from_millis(250);

#[derive(clap::Args)]
pub(super) struct Settings {
    /// Structured diagnostic level (default: info; `COMB_LOG` overrides at startup).
    #[arg(id = "log_level",
         long = "log-level",
         global = true,
         value_parser = ["off",
         "error",
         "warn",
         "info",
         "debug",
         "trace"])]
    level: Option<String>,
    /// Size of each structured log file in MiB (1..1024; default: 10).
    #[arg(id = "log_file_mib",
         long = "log-file-mib",
         global = true,
         value_parser = hive_input::clap::parsers::count(1..=1024))]
    file_mib: Option<usize>,
    /// Total retained structured log files, including the active file (2..64; default: 5).
    #[arg(id = "log_files",
         long = "log-files",
         global = true,
         value_parser = hive_input::clap::parsers::count(2..=64))]
    files: Option<usize>,
}
impl Settings {
    pub fn backend(&self) -> Result<hive_log::Config> {
        let mut config = self.stored()?;
        config.level = environment_level(config.level)?;
        Ok(config)
    }
    pub fn explicit(&self) -> Option<&'static str> {
        if self.level.is_some() {
            Some("--log-level")
        } else if self.file_mib.is_some() {
            Some("--log-file-mib")
        } else if self.files.is_some() {
            Some("--log-files")
        } else {
            None
        }
    }
    pub fn stored(&self) -> Result<hive_log::Config> {
        let level = self
            .level
            .as_deref()
            .unwrap_or("info")
            .parse::<LevelFilter>()
            .map_err(std::io::Error::other)?;
        let config = hive_log::Config {
            level,
            file_bytes: (self.file_mib.unwrap_or(10) as u64) * 1024 * 1024,
            files: self.files.unwrap_or(5),
            ..Default::default()
        };
        config.validate()?;
        Ok(config)
    }
}

pub(super) fn environment_level(default: LevelFilter) -> std::io::Result<LevelFilter> {
    match std::env::var_os("COMB_LOG") {
        None => Ok(default),
        Some(value) => match value.to_str() {
            Some("off") => Ok(LevelFilter::OFF),
            Some("error") => Ok(LevelFilter::ERROR),
            Some("warn") => Ok(LevelFilter::WARN),
            Some("info") => Ok(LevelFilter::INFO),
            Some("debug") => Ok(LevelFilter::DEBUG),
            Some("trace") => Ok(LevelFilter::TRACE),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "COMB_LOG must be off, error, warn, info, debug or trace.",
            )),
        },
    }
}

pub(super) fn service_failure(
    phase: &'static str,
    error: &(dyn std::error::Error + 'static),
) -> std::io::Result<()> {
    let (backend, guard) = hive_log::start_stderr(hive_log::Config::default())?;
    backend.scope(|| {
        tracing::warn!(name: "comb.service_failed",
             phase = phase,
             error = error,
             decision = "preserve_primary_failure");
    });
    guard.finish(DRAIN);
    Ok(())
}

/// Preparation precedes the owned file sink. Scope only this synchronous work so
/// it cannot shadow the server's global subscriber once binding starts.
pub(super) fn preparation<T>(
    config: hive_log::Config,
    work: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let (backend, guard) = hive_log::start_stderr(config)?;
    let result = backend.scope(work);
    guard.finish(DRAIN);
    result
}

pub(super) fn run<T>(work: impl FnOnce() -> Result<T>) -> Result<T> {
    let (level, invalid) = match environment_level(LevelFilter::INFO) {
        Ok(level) => (level, None),
        Err(error) => (LevelFilter::INFO, Some(error)),
    };
    let (backend, guard) = hive_log::start_stderr(hive_log::Config {
        level,
        ..Default::default()
    })?;
    if let Err(cause) = backend.clone().install_global() {
        guard.finish(DRAIN);
        return Err(Error::from(std::io::Error::other(cause)));
    }
    backend.install_panic_hook();
    if let Some(error) = invalid {
        tracing::warn!(name: "comb.level_fallback",
             error = &error as &(dyn std::error::Error + 'static),
             decision = "use_default_cli_level");
    }
    let result = work();
    if let Err(error) = &result {
        error.log_diagnostic();
    }
    // Never turn an accepted mutation into a failed call because diagnostics failed.
    guard.finish(DRAIN);
    result
}
