//! Physical variants include warm, starting, draining and unreaped reservations.
use super::{Acquisition, Registry};
use crate::runtime::{
    provider::registry::{CapacityPressure, EvictionPolicy, begin_capacity_drain},
    scheduler::eviction_priority,
};
use std::{collections::BTreeSet, time::Duration};

impl Acquisition<'_> {
    pub(in crate::runtime::provider) fn variant_room(&self, registry: &Registry) -> bool {
        let mut keys = BTreeSet::new();
        let mut unknown = 0;
        for shard in &registry.shards {
            if shard.auth == self.auth && !shard.capacity_released() {
                keys.insert(shard.process_options.as_str());
            }
        }
        for recovered in &registry.recovered {
            if recovered.reservation.auth == self.auth && !recovered.reservation.gone {
                if let Some(key) = &recovered.process_options {
                    keys.insert(key.as_str());
                } else {
                    unknown += 1;
                }
            }
        }
        keys.contains(self.command.process_options.as_str())
            || keys.len() + unknown < self.pool.settings.max_process_variants
    }

    pub(super) fn drain_variant(&self, registry: &Registry) {
        let eligible = |shard: &&std::sync::Arc<crate::runtime::provider::shard::Shard>| {
            shard.auth == self.auth
                && shard.process_options != self.command.process_options
                && !shard.capacity_released()
        };
        if registry
            .shards
            .iter()
            .filter(eligible)
            .any(|s| s.draining())
        {
            return;
        }
        let minimum = Duration::from_secs(self.pool.settings.cohort_min_seconds);
        let victim = registry
            .shards
            .iter()
            .filter(eligible)
            .filter(|s| {
                !s.exited()
                    && !s.starting()
                    && (s.occupancy().users.is_empty()
                        || s.ready_since
                            .get()
                            .is_some_and(|at| at.elapsed() >= minimum))
            })
            .min_by_key(|s| {
                let occupancy = s.occupancy();
                (
                    !occupancy.users.is_empty(),
                    eviction_priority(
                        occupancy.activity.values().copied(),
                        !occupancy.slots.is_empty(),
                        s.last_used(),
                    ),
                )
            });
        if let Some(victim) = victim {
            // Free a distinct key, rather than retiring one shard and leaving
            // other warm/loaded shards of that same variant charged forever.
            for shard in registry.shards.iter().filter(|s| {
                s.auth == self.auth && s.process_options == victim.process_options && !s.exited()
            }) {
                begin_capacity_drain(
                    shard,
                    EvictionPolicy::Runtime(CapacityPressure::Processes),
                    Some(&self.pool.wake),
                );
            }
        }
    }
}
