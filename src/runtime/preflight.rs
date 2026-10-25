//! Bounded filesystem work with an explicit owner. Jobs contain data, never a Hub
//! or callbacks into the writer. Cancellation drops interest, not worker ownership.
pub(in crate::runtime) mod auth;
mod context;
mod fs_context;
pub(in crate::runtime) mod request;
mod service;
mod worker;
use crate::{
    error::Result,
    runtime::locks::MutexExt,
    state::{Binding, RuntimeLease, State},
};
pub(super) use service::{Owner, Service, Started};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
pub(super) use worker::{ExecutionProfile, Identity, WorkerInput, WorkerProof};

pub(super) const MAX_ATTEMPTS: usize = 8;

#[derive(Default)]
pub(super) struct Leases {
    leases: Mutex<HashMap<uuid::Uuid, Weak<RuntimeLease>>>,
    pub keys: Arc<crate::auth::Keys>,
}
impl Leases {
    pub(super) fn live_count(&self) -> usize {
        self.leases
            .lock_or_panic()
            .values()
            .filter(|lease| lease.strong_count() > 0)
            .count()
    }
    // Called only by the one filesystem owner, so acquisition cannot race itself.
    fn checkout(&self, state: &State, binding: &Binding) -> Result<Arc<RuntimeLease>> {
        let existing = self
            .leases
            .lock_or_panic()
            .get(&binding.id)
            .and_then(Weak::upgrade);
        if let Some(lease) = existing {
            lease.verify(state, binding)?;
            return Ok(lease);
        }
        let lease = Arc::new(state.runtime_lease(binding)?);
        self.leases
            .lock_or_panic()
            .insert(binding.id, Arc::downgrade(&lease));
        Ok(lease)
    }
}
