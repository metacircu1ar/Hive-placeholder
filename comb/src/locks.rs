//! A permanent, per-Comb poison fence. Never resume inconsistent shared state.
use crate::{Error, Result, error::Cause};
use std::{
    ops::{Deref, DerefMut},
    sync::{
        self, Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Default)]
pub(crate) struct Health {
    poisoned: AtomicBool,
}

impl Health {
    pub fn check(&self) -> Result<()> {
        // Pairs with the release store in fence, before any poisoned owner unlocks.
        if self.poisoned.load(Ordering::Acquire) {
            Err(Error::shared_state_poisoned().caused_by(Cause::Poisoned))
        } else {
            Ok(())
        }
    }

    pub fn fence(&self) {
        // One-way publication: ordinary cleanup can never clear this failure.
        self.poisoned.store(true, Ordering::Release);
    }

    pub fn error_or(&self, error: Error) -> Error {
        match self.check() {
            Ok(()) => error,
            Err(mut fenced) => {
                tracing::warn!(name: "comb.failure_fenced",
                     primary = &error as &(dyn std::error::Error + 'static),
                     secondary = &fenced as &(dyn std::error::Error + 'static),
                     decision = "require_recovery");
                fenced.file_committed = error.file_committed;
                fenced.request_id.clone_from(&error.request_id);
                fenced.caused_by(error)
            }
        }
    }

    fn acquired<T>(&self, result: sync::LockResult<T>) -> Result<T> {
        let guard = result.map_err(|poison| {
            self.fence();
            Error::shared_state_poisoned().caused_by(Cause::poisoned(poison))
        })?;
        // Another domain may have failed while this acquisition was waiting.
        self.check()?;
        Ok(guard)
    }
}

pub(crate) struct Mutex<T> {
    inner: sync::Mutex<T>,
    health: Arc<Health>,
}

impl<T> Mutex<T> {
    pub fn new(value: T, health: Arc<Health>) -> Self {
        Self {
            inner: sync::Mutex::new(value),
            health,
        }
    }

    pub fn lock(&self) -> Result<Guard<'_, sync::MutexGuard<'_, T>>> {
        self.health.check()?;
        Ok(Guard {
            inner: self.health.acquired(self.inner.lock())?,
            health: &self.health,
        })
    }

    #[cfg(test)]
    pub fn is_unlocked(&self) -> bool {
        self.inner.try_lock().is_ok()
    }

    #[cfg(test)]
    pub fn into_inner(self) -> Result<T> {
        self.health.check()?;
        self.health.acquired(self.inner.into_inner())
    }
}

pub(crate) struct RwLock<T> {
    inner: sync::RwLock<T>,
    health: Arc<Health>,
}

impl<T> RwLock<T> {
    pub fn new(value: T, health: Arc<Health>) -> Self {
        Self {
            inner: sync::RwLock::new(value),
            health,
        }
    }

    pub fn read(&self) -> Result<sync::RwLockReadGuard<'_, T>> {
        self.health.check()?;
        // Readers cannot mutate the protected value and do not poison on unwind.
        self.health.acquired(self.inner.read())
    }

    pub fn write(&self) -> Result<Guard<'_, sync::RwLockWriteGuard<'_, T>>> {
        self.health.check()?;
        Ok(Guard {
            inner: self.health.acquired(self.inner.write())?,
            health: &self.health,
        })
    }
}

/// Fences before dropping the underlying exclusive guard and waking another owner.
pub(crate) struct Guard<'a, G> {
    inner: G,
    health: &'a Health,
}

impl<G: Deref> Deref for Guard<'_, G> {
    type Target = G::Target;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<G: DerefMut> DerefMut for Guard<'_, G> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<G> Drop for Guard<'_, G> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.health.fence();
        }
    }
}

#[cfg(test)]
mod tests;
