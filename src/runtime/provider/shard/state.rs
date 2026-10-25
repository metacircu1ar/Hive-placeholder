//! Shard occupancy, publication flags, and nonblocking wake transitions.
use super::{
    Activity, HashMap, HashSet, Instant, MutexExt, Ordering, Result, Shard, Write, unavailable,
};
use std::io::ErrorKind;
use std::sync::MutexGuard;
static WAKE_FAILURES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Lock order is registry, then occupancy, then the catalog clock handle.
/// Never acquire the registry while holding this guard; release it before callbacks.
#[derive(Default)]
pub(crate) struct Occupancy {
    // Only successful clean retirement sets idle_since, under the registry fence.
    pub(crate) idle_since: Option<Instant>,
    pub(crate) activity: HashMap<String, Activity>,
    // Lifetime residency survives release; users are only currently attached owners.
    pub(crate) slots: HashSet<String>,
    pub(crate) users: HashSet<String>,
}

impl Shard {
    pub(crate) fn occupancy(&self) -> MutexGuard<'_, Occupancy> {
        self.occupancy.lock_or_panic()
    }

    pub(crate) fn poke(&self) -> Result<()> {
        // The descriptor stays owned by the shard. A shared reference implements Write;
        // independent one-byte wakes need no user-space serialization.
        match (&self.wake).write(&[1]) {
            Ok(_) => Ok(()),
            Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(()),
            Err(cause) => Err(unavailable().with_source(cause)),
        }
    }

    /// A lost hint never certifies exit: the owned loop still polls its stop flag.
    pub(crate) fn wake_owner(&self) {
        if let Err(error) = self.poke()
            && let Some(count) = crate::observability::sample(&WAKE_FAILURES)
        {
            tracing::debug!(name: "provider.wake_failed",
                 provider_id = self.id.as_str(),
                 count = count,
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "retain_owner");
        }
    }

    pub(crate) fn begin_drain(&self) -> bool {
        self.fence()
    }

    fn fence(&self) -> bool {
        // Ownership data stays independently locked; this is not cleanup evidence.
        !self.draining.swap(true, Ordering::AcqRel)
    }

    // Publish an unconditional owner-boundary fence, including repeated drains.
    pub(crate) fn mark_draining(&self) {
        self.draining.store(true, Ordering::Release);
    }

    pub(crate) fn draining(&self) -> bool {
        // Pairs with fence's release and observes the admission fence.
        self.draining.load(Ordering::Acquire)
    }

    pub(crate) fn exited(&self) -> bool {
        // Pairs with mark_exited after the report and final publication work.
        self.exited.load(Ordering::Acquire)
    }

    pub(crate) fn mark_exited(&self) {
        self.exited.store(true, Ordering::Release);
    }

    pub(crate) fn capacity_released(&self) -> bool {
        // Pairs with release_capacity after owned reap or exact process absence.
        self.capacity_released.load(Ordering::Acquire)
    }

    pub(crate) fn release_capacity(&self) {
        self.capacity_released.store(true, Ordering::Release);
    }

    pub(crate) fn starting(&self) -> bool {
        // Pairs with mark_started after the OnceCell catalog is published.
        self.starting.load(Ordering::Acquire)
    }

    pub(crate) fn mark_started(&self) {
        self.starting.store(false, Ordering::Release);
    }

    pub(crate) fn final_events(&self) -> bool {
        // Pairs with publish_final_events before route/watch senders are closed.
        self.final_events.load(Ordering::Acquire)
    }

    pub(crate) fn publish_final_events(&self, readable: bool) {
        self.final_events.store(readable, Ordering::Release);
    }

    pub(crate) fn stop(&self) -> bool {
        // A standalone stop request publishes no payload; the wake schedules a recheck.
        self.stop.load(Ordering::Relaxed)
    }

    pub(crate) fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub(crate) fn steering_unsupported(&self) -> bool {
        // Monotonic process-wide capability latch; no associated payload is published.
        self.steering_unsupported.load(Ordering::Relaxed)
    }

    pub(crate) fn reject_steering(&self) {
        self.steering_unsupported.store(true, Ordering::Relaxed);
    }

    pub(crate) fn abandoned_attach(&self) -> bool {
        // A standalone close-grace request; occupancy remains under its mutex.
        self.abandoned_attach.load(Ordering::Relaxed)
    }

    pub(crate) fn abandon_attach(&self) {
        // Cancellation establishes the fence before the close-grace request.
        self.fence();
        self.abandoned_attach.store(true, Ordering::Relaxed);
    }

    pub(crate) fn require_empty_attach_check(&self) {
        // Publish successful empty retirement to the next attachment, even if the
        // previous release command has not reached the shard thread yet.
        self.empty_attach_check.store(true, Ordering::Release);
    }

    pub(crate) fn take_empty_attach_check(&self) -> bool {
        self.empty_attach_check.swap(false, Ordering::AcqRel)
    }

    #[cfg(test)]
    pub(crate) fn empty_attach_check(&self) -> bool {
        self.empty_attach_check.load(Ordering::Acquire)
    }

    pub(crate) fn last_used(&self) -> u64 {
        // This is an approximate LRU sequence, not a publication or ownership fence.
        self.last_used.load(Ordering::Relaxed)
    }

    pub(crate) fn touch(&self, sequence: u64) {
        self.last_used.store(sequence, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn reset_drain_for_test(&self) {
        self.draining.store(false, Ordering::Relaxed);
    }
}
