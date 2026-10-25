#![allow(dead_code)] // Shared guards serve different plain-CLI and native-fixture subsets.
//! Release gated native fixtures before their temporary homes are removed.

use super::{Fixture, support};
use hive::state::{Lock, ProcessIdentity};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) const PROBE_READY_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const PROBE_EXIT_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const PROBE_POLL_INTERVAL: Duration = Duration::from_millis(10);
pub(super) const RECOVERY_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) struct ProbeRelease {
    signal: Option<support::ReleaseFile>,
    ready: PathBuf,
    identity: Option<ProcessIdentity>,
}
impl ProbeRelease {
    pub(super) fn new(stop: &Path, ready: &Path) -> Self {
        Self {
            signal: Some(support::ReleaseFile::new(stop)),
            ready: ready.to_owned(),
            identity: None,
        }
    }
    pub(super) fn identity(&mut self) -> ProcessIdentity {
        let pid: u32 = fs::read_to_string(&self.ready).unwrap().parse().unwrap();
        let identity = ProcessIdentity::read(pid).unwrap().unwrap();
        self.identity = Some(identity.clone());
        identity
    }
}
impl Drop for ProbeRelease {
    fn drop(&mut self) {
        drop(self.signal.take());
        let identity = self.identity.take().or_else(|| {
            // Readiness can arrive while the test is unwinding its wait. Keep
            // the release file alive until the launched fixture can observe it.
            let pid =
                support::wait::try_wait_until(PROBE_READY_TIMEOUT, PROBE_POLL_INTERVAL, || {
                    fs::read_to_string(&self.ready).ok()?.parse::<u32>().ok()
                })?;
            ProcessIdentity::read(pid).ok().flatten()
        });
        if let Some(identity) = identity {
            let _ = support::wait::try_wait_until(PROBE_EXIT_TIMEOUT, PROBE_POLL_INTERVAL, || {
                identity.absent().ok().filter(|absent| *absent)
            });
        }
    }
}

pub(super) struct LoginRelease<'a> {
    signal: Option<support::ReleaseFile>,
    fixture: &'a Fixture,
}
impl<'a> LoginRelease<'a> {
    pub(super) fn new(stop: &Path, fixture: &'a Fixture) -> Self {
        Self {
            signal: Some(support::ReleaseFile::new(stop)),
            fixture,
        }
    }
}
impl Drop for LoginRelease<'_> {
    fn drop(&mut self) {
        drop(self.signal.take());
        let _ =
            support::wait::try_wait_until(RECOVERY_TIMEOUT, support::wait::POLL_INTERVAL, || {
                let generations = self.fixture.state.generations("claude-main").ok()?;
                for generation in generations {
                    let path = generation.binding.directory.join("auth.lock");
                    if path.exists() {
                        // Successful exclusive acquisition proves that the
                        // detached writer has closed its inherited description.
                        let _released = Lock::acquire(&path).ok()?;
                    }
                }
                Some(())
            });
    }
}
