use super::*;
mod keys;

fn selected(value: ShardSelection) -> Arc<Shard> {
    match value {
        ShardSelection::Selected(shard) => shard,
        ShardSelection::Blocked => panic!("blocked selection"),
        ShardSelection::VariantBlocked => panic!("blocked variant"),
    }
}

use crate::runtime::{
    provider::{Backend, Launch, shard::corpus_support},
    test_support::RuntimeFixture,
};

fn shard(fixture: &RuntimeFixture, name: &str, classes: &[usize]) -> Arc<Shard> {
    let corpus_support::ShardSetup {
        mut shard,
        queues: _queues,
    } = corpus_support::shard(fixture);
    Arc::get_mut(&mut shard).unwrap().id = name.into();
    for (index, &class) in classes.iter().enumerate() {
        let id = format!("{name}-{index}");
        shard.occupancy().slots.insert(id.clone());
        shard.occupancy().activity.insert(
            id,
            Activity {
                class,
                busy: false,
                waiting: true,
            },
        );
    }
    shard
}

#[test]
fn used_capacity_precedes_spares_without_changing_affinity_or_used_order() {
    let fixture = RuntimeFixture::new();
    let mut pool = fixture.pool();
    pool.settings.sessions_per_codex_process = 4;
    let launch = Launch {
        process_options: String::new(),
        compatibility: None,
        command: std::process::Command::new("/unused"),
        launcher: fixture.launcher.clone(),
        backend: Backend::Codex,
        owner: None,
        credential: crate::auth::Credential::default(),
    };
    let large = shard(&fixture, "large", &[2, 2, 2]);
    let spare = shard(&fixture, "spare", &[]);
    let small = shard(&fixture, "small", &[2]);
    let mut registry = Registry {
        probes: 0,
        shards: vec![spare.clone(), large.clone(), small.clone()],
        recovered: vec![],
        unresolved_auth: std::collections::HashSet::new(),
    };
    let selection = Acquisition {
        pool: &pool,
        agent: "next",
        previous_provider: None,
        auth: "main",
        command: &launch,
        lease: &fixture.lease,
        daemon_lock: fixture.daemon_lock.clone(),
        class: 2,
    };
    assert!(Arc::ptr_eq(
        &selection.candidates(&registry).preferred.unwrap(),
        &small
    ));
    // Retained, unsubscribed slots still count, even with no current users.
    assert!(small.occupancy().users.is_empty());
    for index in 1..4 {
        small.occupancy().slots.insert(format!("small-{index}"));
    }
    assert!(Arc::ptr_eq(
        &selection.candidates(&registry).preferred.unwrap(),
        &large
    ));
    large.begin_drain();
    assert!(Arc::ptr_eq(
        &selection.candidates(&registry).preferred.unwrap(),
        &spare
    ));
    large.reset_drain_for_test();
    large
        .occupancy()
        .activity
        .values_mut()
        .for_each(|a| a.class = 1);
    // Preserve affinity: an empty preferred spare still beats a mixed fallback.
    assert!(Arc::ptr_eq(
        &selected(selection.choose(&mut registry).unwrap()),
        &spare
    ));
    let owner = Acquisition {
        agent: "small-0",
        previous_provider: Some("small"),
        ..selection
    };
    assert!(Arc::ptr_eq(
        &selected(owner.choose(&mut registry).unwrap()),
        &small
    ));
    drop(pool);
    fixture.finish();
}

#[test]
fn fallback_keeps_coordinator_protection_and_rejects_incompatible_used_shards() {
    let fixture = RuntimeFixture::new();
    let mut pool = fixture.pool();
    pool.settings.sessions_per_codex_process = 4;
    let launch = Launch::codex(
        std::process::Command::new("/unused"),
        fixture.launcher.clone(),
    );
    let protected = shard(&fixture, "coordinator", &[0]);
    let ordinary = shard(&fixture, "ordinary", &[1, 1]);
    let mut wrong_auth = shard(&fixture, "wrong-auth", &[2]);
    Arc::get_mut(&mut wrong_auth).unwrap().auth = "other".into();
    let registry = Registry {
        probes: 0,
        shards: vec![protected.clone(), wrong_auth, ordinary.clone()],
        recovered: vec![],
        unresolved_auth: std::collections::HashSet::new(),
    };
    let selection = Acquisition {
        pool: &pool,
        agent: "next",
        previous_provider: None,
        auth: "main",
        command: &launch,
        lease: &fixture.lease,
        daemon_lock: fixture.daemon_lock.clone(),
        class: 2,
    };
    let candidates = selection.candidates(&registry);
    assert!(candidates.preferred.is_none());
    assert!(Arc::ptr_eq(&candidates.fallback.unwrap(), &ordinary));
    ordinary.begin_drain();
    assert!(Arc::ptr_eq(
        &selection.candidates(&registry).fallback.unwrap(),
        &protected
    ));
    drop(pool);
    fixture.finish();
}

#[test]
fn repeated_reservations_fill_used_capacity_before_consuming_a_spare() {
    let fixture = RuntimeFixture::new();
    let mut pool = fixture.pool();
    pool.settings.sessions_per_codex_process = 4;
    pool.settings.max_providers = 2;
    pool.settings.max_loaded = 8;
    let launch = Launch::codex(
        std::process::Command::new("/unused"),
        fixture.launcher.clone(),
    );
    let used = shard(&fixture, "used", &[2]);
    let spare = shard(&fixture, "spare", &[]);
    pool.registry.lock_or_panic().shards = vec![spare.clone(), used.clone()];
    for index in 1..=4 {
        let agent = format!("new-{index}");
        let selection = Acquisition {
            pool: &pool,
            agent: &agent,
            previous_provider: None,
            auth: "main",
            command: &launch,
            lease: &fixture.lease,
            daemon_lock: fixture.daemon_lock.clone(),
            class: 2,
        };
        let chosen = selection.try_select().unwrap().unwrap();
        assert!(Arc::ptr_eq(&chosen, if index < 4 { &used } else { &spare }));
        let registry = pool.registry.lock_or_panic();
        assert_eq!(registry.shards.len(), 2);
        assert!(
            registry
                .shards
                .iter()
                .all(|s| s.occupancy().slots.len() <= 4)
        );
    }
    assert_eq!(used.occupancy().slots.len(), 4);
    assert_eq!(spare.occupancy().slots.len(), 1);
    drop(pool);
    fixture.finish();
}

#[test]
fn clean_release_reuses_remaining_slots_and_full_shards_retire() {
    let fixture = RuntimeFixture::new();
    let mut pool = fixture.pool();
    pool.settings.sessions_per_codex_process = 3;
    let used = shard(&fixture, "used", &[2]);
    used.occupancy().users.insert("used-0".into());
    pool.registry.lock_or_panic().shards.push(used.clone());
    assert!(!pool.release_user(&used, "used-0", true));
    assert!(used.occupancy().idle_since.is_some());
    let launch = Launch::codex(
        std::process::Command::new("/unused"),
        fixture.launcher.clone(),
    );
    for agent in ["second", "third"] {
        let selection = Acquisition {
            pool: &pool,
            agent,
            previous_provider: None,
            auth: "main",
            command: &launch,
            lease: &fixture.lease,
            daemon_lock: fixture.daemon_lock.clone(),
            class: 2,
        };
        assert!(Arc::ptr_eq(
            &selection.try_select().unwrap().unwrap(),
            &used
        ));
        assert!(used.occupancy().idle_since.is_none());
        assert_eq!(pool.release_user(&used, agent, true), agent == "third");
    }
    assert_eq!(used.occupancy().slots.len(), 3);
    assert!(used.draining());
    drop(pool);
    fixture.finish();
}

#[test]
fn unclean_or_disabled_release_never_lingers() {
    let fixture = RuntimeFixture::new();
    for (clean, linger, slots) in [(false, 2, 4), (true, 0, 4), (true, 2, 1)] {
        let mut pool = fixture.pool();
        pool.settings.sessions_per_codex_process = slots;
        pool.settings.linger_seconds = linger;
        let used = shard(&fixture, "used", &[2]);
        used.occupancy().users.insert("used-0".into());
        assert!(pool.release_user(&used, "used-0", clean));
        assert!(used.occupancy().idle_since.is_none());
        assert!(used.draining());
    }
    fixture.finish();
}

#[test]
fn historical_owner_never_overrides_current_auth_generation_or_backend() {
    let fixture = RuntimeFixture::new();
    let repaired = RuntimeFixture::new();
    let mut pool = fixture.pool();
    pool.settings.sessions_per_codex_process = 4;
    pool.settings.max_providers = 2;
    let old = shard(&fixture, "old", &[2]);
    let mut compatible = shard(&fixture, "new", &[]);
    Arc::get_mut(&mut compatible).unwrap().auth = "other".into();
    let mut registry = Registry {
        probes: 0,
        shards: vec![old.clone(), compatible.clone()],
        recovered: vec![],
        unresolved_auth: std::collections::HashSet::default(),
    };
    let launch = Launch::codex(
        std::process::Command::new("/unused"),
        fixture.launcher.clone(),
    );
    let selection = Acquisition {
        pool: &pool,
        agent: "old-0",
        previous_provider: None,
        auth: "other",
        command: &launch,
        lease: &fixture.lease,
        daemon_lock: fixture.daemon_lock.clone(),
        class: 2,
    };
    assert!(Arc::ptr_eq(
        &selected(selection.choose(&mut registry).unwrap()),
        &compatible
    ));
    registry.shards.pop();
    drop(compatible);
    let mut replacement = shard(&repaired, "replacement", &[]);
    Arc::get_mut(&mut replacement).unwrap().lease = repaired.lease.clone();
    registry.shards.push(replacement.clone());
    let selection = Acquisition {
        auth: "main",
        lease: &repaired.lease,
        ..selection
    };
    assert!(Arc::ptr_eq(
        &selected(selection.choose(&mut registry).unwrap()),
        &replacement
    ));
    // Neither a profile switch nor a repaired generation frees old reservations.
    assert!(old.occupancy().slots.contains("old-0"));
    drop(registry);
    drop(replacement);
    drop(old);
    drop(pool);
    repaired.finish();
    fixture.finish();
}
