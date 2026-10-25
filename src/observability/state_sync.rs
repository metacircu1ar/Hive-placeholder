//! Time existing durability barriers without changing their ordering or errors.
use super::timing::Timer;
use std::{fs::File, io, path::Path};

#[derive(Clone, Copy)]
pub(crate) enum LedgerPhase {
    Create,
    Authorize,
    Finish,
    Metadata,
}
impl LedgerPhase {
    fn name(self) -> &'static str {
        match self {
            Self::Create => "ledger_create",
            Self::Authorize => "ledger_authorize",
            Self::Finish => "ledger_finish",
            Self::Metadata => "ledger_metadata",
        }
    }
}
pub(crate) fn barrier(file: &File, phase: &'static str, directory: bool) -> io::Result<()> {
    let timer = Timer::start();
    let result = file.sync_all();
    if let Some(m) = timer.measure() {
        tracing::debug!(name: "state.sync", phase = phase, directory = directory,
            duration_ns = m.duration, start_ns = m.start, end_ns = m.end,
            cpu_ns = m.cpu, success = result.is_ok(),
            errno = result.as_ref().err().and_then(io::Error::raw_os_error));
    }
    result
}
pub(crate) fn sync(file: &File, phase: LedgerPhase, directory: bool) -> io::Result<()> {
    let result = barrier(file, phase.name(), directory);
    // After the timer, so that `state.sync` still measures the device.
    #[cfg(feature = "barrier-delay")]
    if result.is_ok() {
        barrier_delay::apply();
    }
    result
}

/// Measurement builds only (storage plan P3): delay each ledger barrier after it
/// succeeds, so that two builds meet the same slow receipt service. Durability is
/// unchanged; only the wait grows.
#[cfg(feature = "barrier-delay")]
pub(crate) mod barrier_delay {
    use crate::error::{Error, Result};
    use std::{sync::OnceLock, time::Duration};

    /// Whole milliseconds, 0–1000.
    static DELAY: OnceLock<u64> = OnceLock::new();

    /// Read once at daemon start; an invalid value refuses the start.
    pub(crate) fn configure() -> Result<()> {
        let millis = match std::env::var_os("HIVE_BARRIER_DELAY_MS") {
            None => 0,
            Some(value) => parse(value.to_str()).ok_or_else(|| {
                Error::config_invalid(
                    "HIVE_BARRIER_DELAY_MS must be a whole number of milliseconds from 0 to 1000.",
                )
            })?,
        };
        let millis = *DELAY.get_or_init(|| millis);
        tracing::info!(name: "state.barrier_delay", duration_ms = millis);
        Ok(())
    }
    pub(super) fn parse(value: Option<&str>) -> Option<u64> {
        let millis: u64 = value?.parse().ok()?;
        (millis <= 1000).then_some(millis)
    }
    pub(super) fn apply() {
        if let Some(&millis) = DELAY.get().filter(|millis| **millis != 0) {
            std::thread::sleep(Duration::from_millis(millis));
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn delays_are_whole_milliseconds_up_to_one_second() {
            for (value, expected) in [
                (Some("0"), Some(0)),
                (Some("20"), Some(20)),
                (Some("1000"), Some(1000)),
                (Some("1001"), None),
                (Some("-1"), None),
                (Some("1.5"), None),
                (Some(""), None),
                (None, None),
            ] {
                assert_eq!(super::parse(value), expected, "{value:?}");
            }
        }
    }
}
pub(crate) fn write_json(
    path: &Path,
    value: &impl serde::Serialize,
    phase: LedgerPhase,
) -> crate::error::Result<()> {
    crate::state::write_json_with_sync(path, value, |file, directory| sync(file, phase, directory))
}
