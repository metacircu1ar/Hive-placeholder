//! Fixed-size accounting for expected transport loss; never retain rejected payloads.
pub(crate) mod state_sync;
pub(crate) mod timing;

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy)]
pub(crate) enum ReplyOwner {
    Writer,
    Reader,
    Provider,
    Preflight,
    Control,
    Shutdown,
    Collector,
    Auth,
    Shell,
}
impl ReplyOwner {
    const ALL: [Self; 9] = [
        Self::Writer,
        Self::Reader,
        Self::Provider,
        Self::Preflight,
        Self::Control,
        Self::Shutdown,
        Self::Collector,
        Self::Auth,
        Self::Shell,
    ];
    fn name(self) -> &'static str {
        match self {
            Self::Writer => "writer",
            Self::Reader => "reader",
            Self::Provider => "provider",
            Self::Preflight => "preflight",
            Self::Control => "control",
            Self::Shutdown => "shutdown",
            Self::Collector => "collector",
            Self::Auth => "auth",
            Self::Shell => "shell",
        }
    }
}

static LOST: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];
static OPTIONAL_FAILURES: AtomicU64 = AtomicU64::new(0);

/// Cleanup always runs, but cannot erase the operation's error. Its own cause
/// remains visible even when the caller can return only one error.
pub(crate) fn settle<T>(
    primary: crate::error::Result<T>,
    cleanup: crate::error::Result<()>,
    phase: &'static str,
) -> crate::error::Result<T> {
    if let Err(secondary) = cleanup {
        if let Err(primary) = &primary {
            tracing::warn!(name: "cleanup.secondary_failed",
                 phase = phase,
                 primary = primary as &(dyn std::error::Error + 'static),
                 secondary = &secondary as &(dyn std::error::Error + 'static),
                 decision = "preserve_primary_failure");
        } else {
            return Err(secondary);
        }
    }
    primary
}

/// Optional observations must never certify ownership or authorize a mutation.
/// The caller explicitly chooses to continue without this auxiliary value.
pub(crate) fn optional<T, E: std::error::Error + 'static>(
    result: Result<T, E>,
    phase: &'static str,
) -> Option<T> {
    result
        .inspect_err(|error| observation_failed(phase, error))
        .ok()
}

pub(crate) fn observation_failed(phase: &'static str, error: &(dyn std::error::Error + 'static)) {
    if let Some(count) = sample(&OPTIONAL_FAILURES) {
        tracing::debug!(name: "observation.unavailable",
             phase = phase,
             count = count,
             error = error,
             decision = "omit_optional_observation");
    }
}

/// A send error owns the undelivered private payload, not an error cause. Count
/// and drop it without formatting, retaining, retrying or changing accepted work.
pub(crate) fn reply<T, E>(result: Result<T, E>, owner: ReplyOwner) {
    if result.is_err() {
        lost_reply(owner, None);
    }
}

pub(crate) fn lost_reply(owner: ReplyOwner, error: Option<&(dyn std::error::Error + 'static)>) {
    // Exponential sampling is bounded to at most 64 records per owner lifetime.
    // Shutdown reports the exact cumulative total, including unsampled losses.
    if let Some(count) = sample(&LOST[owner as usize]) {
        if let Some(error) = error {
            tracing::debug!(name: "reply.lost",
                 phase = owner.name(),
                 count = count,
                 error = error,
                 decision = "do_not_replay");
        } else {
            tracing::debug!(name: "reply.lost",
                 phase = owner.name(),
                 count = count,
                 decision = "requester_gone");
        }
    }
}

/// Fixed-size counters keep repeated failures from allocating diagnostic state.
pub(crate) fn sample(counter: &AtomicU64) -> Option<u64> {
    let previous = counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .ok()?;
    let count = previous + 1;
    count.is_power_of_two().then_some(count)
}

pub(crate) fn finish() {
    let optional = OPTIONAL_FAILURES.load(Ordering::Relaxed);
    if optional > 0 {
        tracing::info!(name: "observation.unavailable_total", count = optional);
    }
    for owner in ReplyOwner::ALL {
        let count = LOST[owner as usize].load(Ordering::Relaxed);
        if count > 0 {
            tracing::info!(name: "reply.loss_total", phase = owner.name(), count = count);
        }
    }
}
