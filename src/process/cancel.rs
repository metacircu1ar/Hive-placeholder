//! Cancellation checkpoints shared by owned provider operations.
use super::{Arc, AtomicUsize, Error, Ordering, Result, os};

#[derive(Clone)]
pub(crate) struct Cancellation {
    pub(super) signals: Arc<AtomicUsize>,
    pub(super) checkpoint: usize,
}
impl Default for Cancellation {
    fn default() -> Self {
        Self {
            signals: Arc::new(AtomicUsize::new(0)),
            checkpoint: 0,
        }
    }
}
impl Cancellation {
    pub(crate) fn install() -> Result<Self> {
        let token = Self::default();
        os::install_cancellation(&token.signals).map_err(Error::state_io_from)?;
        Ok(token)
    }
    pub(crate) fn count(&self) -> usize {
        self.signals
            .load(Ordering::Relaxed)
            .saturating_sub(self.checkpoint)
    }
    #[cfg(test)]
    pub(crate) fn request(&self) {
        self.signals.fetch_add(1, Ordering::Relaxed);
    }
    #[must_use]
    pub(crate) fn checkpoint(&self) -> Self {
        Self {
            signals: self.signals.clone(),
            checkpoint: self.signals.load(Ordering::Relaxed),
        }
    }
}
