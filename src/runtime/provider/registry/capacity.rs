//! Account provider headroom and select eviction candidates under the registry fence.
use super::Registry;
use crate::{
    config::Runtime,
    runtime::{provider::shard::Shard, scheduler::eviction_priority},
};
use std::{sync::Arc, time::Duration};

#[derive(Debug)]
pub(crate) struct CapacityRoom {
    pub process: bool,
    pub starting: bool,
    pub resident: usize,
}
impl CapacityRoom {
    pub(crate) fn can_start(&self) -> bool {
        self.process && self.starting
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CapacityPressure {
    Actors,
    Processes,
}

pub(in crate::runtime::provider) fn begin_capacity_drain(
    shard: &Shard,
    policy: EvictionPolicy,
    wake: Option<&crate::runtime::hub::Wake>,
) {
    if shard.begin_drain() {
        let phase = match policy {
            EvictionPolicy::Runtime(CapacityPressure::Actors) => "actor_pressure",
            EvictionPolicy::Runtime(CapacityPressure::Processes) => "process_pressure",
            EvictionPolicy::ColdDiscovery => "cold_discovery",
        };
        tracing::info!(name: "provider.capacity_drain",
             provider_id = shard.id.as_str(),
             phase = phase);
    }
    shard.wake_owner();
    if let Some(wake) = wake {
        wake.all();
    }
}

#[derive(Clone, Copy)]
pub(crate) enum EvictionPolicy {
    Runtime(CapacityPressure),
    ColdDiscovery,
}
pub(crate) enum Eviction<'a> {
    InFlight,
    Victim(&'a Arc<Shard>),
    NoVictim,
}

impl Registry {
    pub(crate) fn room(&self, settings: &Runtime) -> CapacityRoom {
        let recovered = self.recovered_capacity();
        CapacityRoom {
            process: self.shards.len() + recovered.processes + self.probes < settings.max_providers,
            starting: self.shards.iter().filter(|shard| shard.starting()).count() + self.probes
                < settings.max_starting,
            resident: recovered.slots
                + self
                    .shards
                    .iter()
                    .map(|shard| shard.occupancy().slots.len())
                    .sum::<usize>(),
        }
    }

    pub(crate) fn charge_probe(&mut self, settings: &Runtime) -> Result<(), CapacityRoom> {
        let room = self.room(settings);
        if !room.can_start() {
            return Err(room);
        }
        self.probes += 1;
        Ok(())
    }

    pub(crate) fn release_probe(&mut self) {
        self.probes -= 1;
    }

    pub(crate) fn eviction_victim(
        &self,
        policy: EvictionPolicy,
        cohort_minimum: Duration,
    ) -> Eviction<'_> {
        // A demand eviction waits only for a live drain. Cold discovery retains
        // the stricter fence for an exited-but-unremoved draining reservation.
        if self.shards.iter().any(|shard| {
            shard.draining() && (matches!(policy, EvictionPolicy::ColdDiscovery) || !shard.exited())
        }) {
            return Eviction::InFlight;
        }
        let eligible = self.shards.iter().filter(|shard| {
            if shard.exited() {
                return false;
            }
            let occupancy = shard.occupancy();
            match policy {
                EvictionPolicy::ColdDiscovery => {
                    // Discovery must never displace attached work or a dirty
                    // retired cohort: only a spare or clean lingering process.
                    occupancy.users.is_empty()
                        && (occupancy.slots.is_empty() || occupancy.idle_since.is_some())
                }
                EvictionPolicy::Runtime(reason) => {
                    (reason != CapacityPressure::Actors || !occupancy.users.is_empty())
                        && (occupancy.users.is_empty()
                            || shard
                                .ready_since
                                .get()
                                .is_some_and(|at| at.elapsed() >= cohort_minimum))
                }
            }
        });
        // Preserve discovery's insertion-order choice. Runtime pressure protects
        // affinity/coordinators and ranks occupancy/activity/LRU as before.
        let victim = match policy {
            EvictionPolicy::ColdDiscovery => eligible.into_iter().next(),
            EvictionPolicy::Runtime(_) => eligible.min_by_key(|shard| {
                let occupancy = shard.occupancy();
                (
                    !occupancy.users.is_empty(),
                    eviction_priority(
                        occupancy.activity.values().copied(),
                        !occupancy.slots.is_empty(),
                        shard.last_used(),
                    ),
                )
            }),
        };
        victim.map_or(Eviction::NoVictim, Eviction::Victim)
    }
}

#[cfg(test)]
mod tests;
