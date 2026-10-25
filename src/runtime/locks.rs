//! Shared runtime state is not safe to resume after a panic while it is locked.
//! Keep std's poisoning and unwind the current thread/task on the next lock
//! attempt. Existing provider `catch_unwind/quarantine` and thread-join handling
//! remain responsible for that failure; do not recover the poisoned state or
//! abort the process here.

use std::sync::{Mutex, MutexGuard};

/// A receiver method keeps each guard in the caller's original expression scope,
/// including match/if-let scrutinees, call arguments, and statement temporaries.
pub(super) trait MutexExt<T: ?Sized> {
    fn lock_or_panic(&self) -> MutexGuard<'_, T>;
}

impl<T: ?Sized> MutexExt<T> for Mutex<T> {
    #[track_caller]
    #[expect(
        clippy::expect_used,
        reason = "Poisoning must unwind to avoid resuming inconsistent shared runtime state."
    )]
    fn lock_or_panic(&self) -> MutexGuard<'_, T> {
        self.lock().expect("runtime mutex was poisoned")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Test fixtures and assertions deliberately panic on failure.
mod tests {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    #[test]
    fn poison_unwinds_without_resuming_or_clearing_shared_state() {
        let mutex = Mutex::new(0);
        let failed_owner = catch_unwind(AssertUnwindSafe(|| {
            let mut state = mutex.lock_or_panic();
            *state = 1;
            panic!("interrupt shared-state update");
        }));
        assert!(failed_owner.is_err());
        assert!(mutex.is_poisoned());

        let next_owner = catch_unwind(AssertUnwindSafe(|| {
            let _state = mutex.lock_or_panic();
        }));
        assert!(next_owner.is_err());
        assert!(mutex.is_poisoned());
    }
}
