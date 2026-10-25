//! Independent WAL snapshots and the short durable-publication gate.
use crate::locks::{Guard, Health, Mutex};
use crate::{Error, Result, index::Index};
use std::sync::{Arc, MutexGuard};

mod publishing;
pub(crate) use publishing::{FileFailure, Publishing};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PublicationState {
    Ready,
    Pending,
}

pub(crate) struct Publication(Mutex<PublicationState>);
impl Publication {
    pub fn new(health: Arc<Health>) -> Self {
        Self(Mutex::new(PublicationState::Ready, health))
    }
    pub fn lock(&self) -> Result<Guard<'_, MutexGuard<'_, PublicationState>>> {
        self.0.lock()
    }
    pub fn fail(&self) {
        match self.0.lock() {
            Ok(mut state) => *state = PublicationState::Pending,
            Err(error) => {
                tracing::error!(name: "comb.publication_fenced",
                     error = &error as &(dyn std::error::Error + 'static),
                     decision = "require_recovery");
            }
        }
    }
}
pub(crate) struct Snapshot {
    pub index: Index,
}
impl Snapshot {
    pub fn pin(index: Index, publication: &Arc<Publication>) -> Result<Self> {
        let guard = publication.lock()?;
        if *guard == PublicationState::Pending {
            return Err(Error::pending_state());
        }
        index.pin_snapshot()?;
        drop(guard);
        Ok(Self { index })
    }
}
impl Drop for Snapshot {
    fn drop(&mut self) {
        if let Err(error) = self.index.rollback_snapshot() {
            tracing::error!(name: "comb.snapshot_release_failed",
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "discard_reader");
        }
    }
}
