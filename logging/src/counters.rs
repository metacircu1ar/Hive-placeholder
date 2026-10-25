use serde::Serialize;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

#[derive(Default)]
pub(crate) struct Counters {
    #[cfg(test)]
    pub writer_iterations: AtomicU64,
    #[cfg(test)]
    pub fallback_iterations: AtomicU64,
    pub fatal_unconfirmed: AtomicU64,
    pub full: AtomicU64,
    pub disconnected: AtomicU64,
    pub invalid: AtomicU64,
    pub numeric_invalid: AtomicU64,
    pub oversized: AtomicU64,
    pub write_failed: AtomicU64,
    pub finish_failed: AtomicU64,
    pub fallback_failed: AtomicU64,
    pub written: AtomicU64,
    pub errno: AtomicI32,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct Counts {
    pub fatal_unconfirmed: u64,
    pub full: u64,
    pub disconnected: u64,
    pub invalid: u64,
    /// Integer overflow or a non-finite float; included in `invalid` too.
    pub numeric_invalid: u64,
    pub oversized: u64,
    pub write_failed: u64,
    pub finish_failed: u64,
    pub fallback_failed: u64,
    /// Process-wide atomic-filesystem destructor failures, including other owners.
    pub filesystem_cleanup_failed: u64,
    pub written: u64,
    pub last_errno: Option<i32>,
}
impl Counters {
    pub fn snapshot(&self) -> Counts {
        let errno = self.errno.load(Ordering::Relaxed);
        Counts {
            fatal_unconfirmed: self.fatal_unconfirmed.load(Ordering::Relaxed),
            full: self.full.load(Ordering::Relaxed),
            disconnected: self.disconnected.load(Ordering::Relaxed),
            invalid: self.invalid.load(Ordering::Relaxed),
            numeric_invalid: self.numeric_invalid.load(Ordering::Relaxed),
            oversized: self.oversized.load(Ordering::Relaxed),
            write_failed: self.write_failed.load(Ordering::Relaxed),
            finish_failed: self.finish_failed.load(Ordering::Relaxed),
            fallback_failed: self.fallback_failed.load(Ordering::Relaxed),
            filesystem_cleanup_failed: atomic_fs::destructor_cleanup_failures(),
            written: self.written.load(Ordering::Relaxed),
            last_errno: (errno != 0).then_some(errno),
        }
    }
}
