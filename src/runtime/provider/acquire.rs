//! Wait for provider capacity and choose a compatible shard.
use super::registry::CapacityPressure;
use super::{error::unavailable, pool::Pool, registry::Registry, shard::Shard};
use crate::{
    error::Result,
    runtime::{locks::MutexExt, scheduler::Activity},
    state::{Lock, RuntimeLease},
};
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

// Retry blocked acquisition periodically even if no capacity notification arrives.
const ACQUISITION_RECHECK_INTERVAL: Duration = Duration::from_secs(1);
mod variants;

struct ShardCandidates {
    preferred: Option<Arc<Shard>>,
    fallback: Option<Arc<Shard>>,
}

enum ShardSelection {
    Selected(Arc<Shard>),
    Blocked,
    VariantBlocked,
}

pub(super) struct Acquisition<'a> {
    pub(super) pool: &'a Pool,
    pub(super) agent: &'a str,
    pub(super) previous_provider: Option<&'a str>,
    pub(super) auth: &'a str,
    pub(super) command: &'a super::Launch,
    pub(super) lease: &'a Arc<RuntimeLease>,
    pub(super) daemon_lock: Arc<Lock>,
    pub(super) class: usize,
}
impl Acquisition<'_> {
    pub(super) async fn wait(&self) -> Result<Arc<Shard>> {
        // Keep the command until a new shard is actually necessary.
        loop {
            if self.pool.shutdown.load(Ordering::Relaxed) {
                return Err(unavailable());
            }
            let notified = self.pool.changed.notified();
            let selected = self.try_select()?;
            if let Some(shard) = selected {
                return Ok(shard);
            }
            tokio::select! {
                () = notified => {},
                () = tokio::time::sleep(ACQUISITION_RECHECK_INTERVAL) => {},
            }
        }
    }

    fn try_select(&self) -> Result<Option<Arc<Shard>>> {
        if let Some(proof) = &self.command.compatibility {
            proof.check()?;
        }
        if !self.command.credential.fresh() {
            return Err(crate::state::key_changed());
        }
        let mut registry = self.pool.registry.lock_or_panic();
        if self.pool.shutdown.load(Ordering::Relaxed) || self.pool.uncertain.load(Ordering::Relaxed)
        {
            return Err(unavailable());
        }
        registry.refresh();
        match self.choose(&mut registry)? {
            ShardSelection::Selected(s) => {
                let mut occupancy = s.occupancy();
                occupancy.idle_since = None;
                occupancy.slots.insert(self.agent.into());
                occupancy.users.insert(self.agent.into());
                occupancy.activity.insert(
                    self.agent.into(),
                    Activity {
                        class: self.class,
                        busy: false,
                        waiting: true,
                    },
                );
                drop(occupancy);
                self.start_spares(&mut registry);
                Ok(Some(s))
            }
            ShardSelection::Blocked => {
                self.pool.drain_one(&registry, CapacityPressure::Processes);
                Ok(None)
            }
            ShardSelection::VariantBlocked => {
                self.drain_variant(&registry);
                Ok(None)
            }
        }
    }

    fn choose(&self, registry: &mut Registry) -> Result<ShardSelection> {
        let owner = registry
            .shards
            .iter()
            .find(|s| {
                self.previous_provider == Some(s.id.as_str())
                    && s.auth == self.auth
                    && s.process_options == self.command.process_options
                    && s.key.as_ref() == self.command.credential.authority()
                    && s.lease.generation() == self.lease.generation()
                    && self.same_executable(s)
                    && s.backend.multiplexed()
                    && self.command.backend.multiplexed()
                    && s.occupancy().slots.contains(self.agent)
            })
            .cloned();
        let room = registry.room(&self.pool.settings);
        let variant_room = self.variant_room(registry);
        let can_start = room.can_start() && variant_room;
        let ShardCandidates {
            preferred,
            fallback,
        } = self.candidates(registry);
        let selected = if let Some(owner) = owner {
            let draining = owner.draining();
            (!draining).then_some(owner)
        } else if room.resident < self.pool.settings.max_loaded {
            if let Some(s) = preferred {
                Some(s)
            } else if can_start {
                Some(self.pool.start_shard(
                    registry,
                    self.auth,
                    self.command,
                    self.lease.clone(),
                    self.daemon_lock.clone(),
                )?)
            } else {
                fallback
            }
        } else {
            None
        };
        Ok(selected.map_or(
            if variant_room {
                ShardSelection::Blocked
            } else {
                ShardSelection::VariantBlocked
            },
            ShardSelection::Selected,
        ))
    }

    fn candidates(&self, registry: &Registry) -> ShardCandidates {
        let compatible = |s: &&Arc<Shard>| {
            if s.auth != self.auth
                || s.process_options != self.command.process_options
                || s.key.as_ref() != self.command.credential.authority()
                || s.lease.generation() != self.lease.generation()
                || !self.same_executable(s)
                || !self.command.backend.multiplexed()
                || !s.backend.multiplexed()
                || s.draining()
            {
                return false;
            }
            // A fresh conversation for this identity needs a fresh lifetime
            // slot. Old thread routing remains until process retirement.
            let occupancy = s.occupancy();
            !occupancy.slots.contains(self.agent)
                && occupancy.slots.len() < self.pool.settings.sessions_per_codex_process
        };
        let preferred = registry
            .shards
            .iter()
            .filter(compatible)
            .filter(|s| {
                s.occupancy()
                    .activity
                    .values()
                    .all(|a| a.class == self.class)
            })
            .min_by_key(|s| {
                // Consume a spare only after used capacity in this affinity tier.
                // Otherwise each assignment replenishes an empty preferred shard
                // and spreads conversations across the entire physical pool.
                let occupancy = s.occupancy();
                (occupancy.slots.is_empty(), occupancy.slots.len())
            })
            .cloned();
        let fallback = registry
            .shards
            .iter()
            .filter(compatible)
            .min_by_key(|s| {
                let occupancy = s.occupancy();
                let coordinator = occupancy.activity.values().any(|a| a.class == 0);
                (
                    coordinator,
                    occupancy.slots.is_empty(),
                    occupancy.slots.len(),
                )
            })
            .cloned();
        ShardCandidates {
            preferred,
            fallback,
        }
    }

    pub(super) fn same_executable(&self, shard: &Shard) -> bool {
        match (&self.command.compatibility, &shard.compatibility) {
            (Some(selected), Some(resident)) => selected.identity == resident.identity,
            // Direct transport fixtures have no executable admission layer.
            (None, None) => cfg!(test),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests;
