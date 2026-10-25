//! Capacity and eviction regressions for the distinct runtime and discovery policies.
use super::*;
use crate::runtime::{
    provider::shard::corpus_support::{ShardSetup, shard},
    test_support::RuntimeFixture,
};
use std::{collections::HashSet, time::Instant};

fn registry(shards: Vec<Arc<Shard>>) -> Registry {
    Registry {
        probes: 0,
        shards,
        recovered: Vec::new(),
        unresolved_auth: HashSet::new(),
    }
}
fn victim(registry: &Registry, policy: EvictionPolicy) -> &Arc<Shard> {
    match registry.eviction_victim(policy, Duration::ZERO) {
        Eviction::Victim(shard) => shard,
        Eviction::InFlight => panic!("unexpected in-flight drain"),
        Eviction::NoVictim => panic!("expected an eligible victim"),
    }
}

#[test]
fn probe_charge_checks_both_limits_and_failure_does_not_reserve_capacity() {
    let fixture = RuntimeFixture::new();
    let mut settings = fixture.pool().settings;
    let mut registry = registry(Vec::new());
    settings.max_providers = 2;
    settings.max_starting = 1;
    registry.charge_probe(&settings).unwrap();
    let blocked = registry.charge_probe(&settings).unwrap_err();
    assert!(blocked.process);
    assert!(!blocked.starting);
    assert_eq!(registry.probes, 1);
    registry.release_probe();
    settings.max_providers = 1;
    settings.max_starting = 2;
    registry.charge_probe(&settings).unwrap();
    let blocked = registry.charge_probe(&settings).unwrap_err();
    assert!(!blocked.process);
    assert!(blocked.starting);
    assert_eq!(registry.probes, 1);
    registry.release_probe();
    assert!(registry.room(&settings).can_start());
    fixture.finish();
}

#[test]
fn discovery_preserves_first_clean_candidate_while_runtime_ranks_lru() {
    let fixture = RuntimeFixture::new();
    let ShardSetup {
        shard: first,
        queues: _first_queues,
    } = shard(&fixture);
    let ShardSetup {
        shard: older,
        queues: _older_queues,
    } = shard(&fixture);
    first.touch(20);
    older.touch(1);
    let registry = registry(vec![first.clone(), older.clone()]);
    assert!(Arc::ptr_eq(
        victim(&registry, EvictionPolicy::ColdDiscovery),
        &first
    ));
    assert!(Arc::ptr_eq(
        victim(
            &registry,
            EvictionPolicy::Runtime(CapacityPressure::Processes)
        ),
        &older,
    ));
    assert!(matches!(
        registry.eviction_victim(
            EvictionPolicy::Runtime(CapacityPressure::Actors),
            Duration::ZERO
        ),
        Eviction::NoVictim,
    ));
    drop(registry);
    drop(first);
    drop(older);
    fixture.finish();
}

#[test]
fn discovery_requires_clean_linger_and_never_displaces_an_attached_owner() {
    let fixture = RuntimeFixture::new();
    let ShardSetup {
        shard,
        queues: _queues,
    } = shard(&fixture);
    shard.occupancy().slots.insert("retired".into());
    let registry = registry(vec![shard.clone()]);
    assert!(matches!(
        registry.eviction_victim(EvictionPolicy::ColdDiscovery, Duration::ZERO),
        Eviction::NoVictim,
    ));
    assert!(Arc::ptr_eq(
        victim(
            &registry,
            EvictionPolicy::Runtime(CapacityPressure::Processes)
        ),
        &shard,
    ));
    shard.occupancy().idle_since = Some(Instant::now());
    assert!(Arc::ptr_eq(
        victim(&registry, EvictionPolicy::ColdDiscovery),
        &shard
    ));
    shard.occupancy().users.insert("attached".into());
    assert!(matches!(
        registry.eviction_victim(EvictionPolicy::ColdDiscovery, Duration::ZERO),
        Eviction::NoVictim,
    ));
    drop(registry);
    drop(shard);
    fixture.finish();
}

#[test]
fn exited_unremoved_drain_blocks_discovery_but_does_not_hide_a_runtime_victim() {
    let fixture = RuntimeFixture::new();
    let ShardSetup {
        shard: exited,
        queues: _exited_queues,
    } = shard(&fixture);
    let ShardSetup {
        shard: candidate,
        queues: _candidate_queues,
    } = shard(&fixture);
    exited.begin_drain();
    let registry = registry(vec![exited.clone(), candidate.clone()]);
    for policy in [
        EvictionPolicy::ColdDiscovery,
        EvictionPolicy::Runtime(CapacityPressure::Processes),
    ] {
        assert!(matches!(
            registry.eviction_victim(policy, Duration::ZERO),
            Eviction::InFlight
        ));
    }
    exited.mark_exited();
    assert!(matches!(
        registry.eviction_victim(EvictionPolicy::ColdDiscovery, Duration::ZERO),
        Eviction::InFlight,
    ));
    assert!(Arc::ptr_eq(
        victim(
            &registry,
            EvictionPolicy::Runtime(CapacityPressure::Processes)
        ),
        &candidate,
    ));
    assert!(!exited.capacity_released());
    drop(registry);
    drop(exited);
    drop(candidate);
    fixture.finish();
}
