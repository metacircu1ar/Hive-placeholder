//! Fixed-size cleanup evidence from the trusted helper. Never transport error text.
use serde::{Deserialize, Serialize};
use std::{error::Error, io, sync::atomic::AtomicU64};

#[derive(Clone, Copy)]
pub(super) enum Phase {
    Operation,
    Ownership,
    Signal,
    Reap,
}
const PHASES: [&str; 4] = ["operation", "ownership", "signal", "reap"];
static REPORTS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Failure {
    count: u64,
    errno: Option<i32>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct Diagnostics([Failure; 4]);

impl Diagnostics {
    pub(super) fn record(&mut self, phase: Phase, error: &(dyn Error + 'static)) {
        let entry = &mut self.0[phase as usize];
        entry.count = entry.count.saturating_add(1);
        // Retain native causes only, bounding potentially cyclic source chains.
        let errno = std::iter::successors(Some(error), |&cause| cause.source())
            .take(8)
            .find_map(|cause| {
                cause
                    .downcast_ref::<io::Error>()
                    .and_then(io::Error::raw_os_error)
            });
        // A later expected signal miss must not hide an earlier actual failure.
        if !matches!(phase, Phase::Signal)
            || entry.count == 1
            || entry.errno == Some(libc::ESRCH)
            || errno != Some(libc::ESRCH)
        {
            entry.errno = errno;
        }
    }

    pub(super) fn log(&self, pid: u32) {
        for (index, entry) in self.0.iter().enumerate() {
            if entry.count > 0 && crate::observability::sample(&REPORTS[index]).is_some() {
                if index == Phase::Signal as usize && entry.errno == Some(libc::ESRCH) {
                    tracing::debug!(name: "shell.helper_already_exited",
                         phase = PHASES[index],
                         pid = pid,
                         count = entry.count,
                         errno = entry.errno,
                         decision = "continue_absence_check");
                } else {
                    tracing::warn!(name: "shell.helper_cleanup_failed",
                         phase = PHASES[index],
                         pid = pid,
                         count = entry.count,
                         errno = entry.errno,
                         decision = "require_cleanup_proof");
                }
            }
        }
    }
}
