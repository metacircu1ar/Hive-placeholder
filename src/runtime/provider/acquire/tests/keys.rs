use super::*;

fn selected_keys(fixture: &RuntimeFixture) -> Vec<crate::auth::Credential> {
    let root = fixture.temp.path().canonicalize().unwrap();
    let path = root.join("key");
    let state = crate::state::State::new(root.join("key-state")).unwrap();
    let binding = state
        .register(
            "api",
            &crate::config::Profile {
                provider: crate::config::Provider::Codex,
                method: crate::config::Method::ApiKey,
                api_key_env: None,
                api_key_file: Some(path.clone()),
            },
        )
        .unwrap();
    let keys = crate::auth::Keys::default();
    [b"a", b"b", b"a"]
        .into_iter()
        .map(|bytes| {
            atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
                .write(&path, bytes)
                .unwrap();
            keys.resolve(&state, &binding).unwrap()
        })
        .collect()
}

#[test]
fn rotation_excludes_old_warm_and_historical_shards_even_when_digest_returns() {
    let fixture = RuntimeFixture::new();
    let keys = selected_keys(&fixture);
    let mut pool = fixture.pool();
    pool.settings.max_providers = 3;
    pool.settings.sessions_per_codex_process = 4;
    let mut old = shard(&fixture, "old", &[2]);
    Arc::get_mut(&mut old).unwrap().key = keys[0].authority().cloned();
    let mut spare = shard(&fixture, "spare", &[]);
    Arc::get_mut(&mut spare).unwrap().key = keys[1].authority().cloned();
    let mut current = shard(&fixture, "current", &[]);
    Arc::get_mut(&mut current).unwrap().key = keys[2].authority().cloned();
    let mut registry = Registry {
        probes: 0,
        shards: vec![old.clone(), spare, current.clone()],
        recovered: vec![],
        unresolved_auth: std::collections::HashSet::default(),
    };
    let mut launch = Launch::codex(
        std::process::Command::new("/unused"),
        fixture.launcher.clone(),
    );
    launch.credential = keys[2].clone();
    let selection = Acquisition {
        pool: &pool,
        agent: "old-0",
        previous_provider: Some("old"),
        auth: "main",
        command: &launch,
        lease: &fixture.lease,
        daemon_lock: fixture.daemon_lock.clone(),
        class: 2,
    };
    assert_eq!(keys[0].fingerprint(), keys[2].fingerprint());
    assert!(Arc::ptr_eq(
        &selected(selection.choose(&mut registry).unwrap()),
        &current
    ));
    assert!(old.occupancy().slots.contains("old-0"));
    launch.credential = keys[0].clone();
    let stale = Acquisition {
        pool: &pool,
        agent: "old-0",
        previous_provider: Some("old"),
        auth: "main",
        command: &launch,
        lease: &fixture.lease,
        daemon_lock: fixture.daemon_lock.clone(),
        class: 2,
    };
    assert!(crate::state::is_key_changed(
        &stale.try_select().err().unwrap()
    ));
    drop(pool);
    fixture.finish();
}

#[test]
fn process_variants_exclude_old_owner_and_spares_and_drain_before_a_new_key() {
    let fixture = RuntimeFixture::new();
    let mut pool = fixture.pool();
    pool.settings.max_process_variants = 1;
    pool.settings.cohort_min_seconds = 0;
    let old = shard(&fixture, "old", &[2]);
    let spare = shard(&fixture, "spare", &[]);
    let mut matching = shard(&fixture, "matching", &[]);
    Arc::get_mut(&mut matching).unwrap().process_options = "new".into();
    let mut registry = Registry {
        probes: 0,
        shards: vec![old.clone(), spare.clone()],
        recovered: vec![],
        unresolved_auth: std::collections::HashSet::default(),
    };
    let mut launch = Launch::codex(
        std::process::Command::new("/unused"),
        fixture.launcher.clone(),
    );
    launch.process_options = "new".into();
    let selection = Acquisition {
        pool: &pool,
        agent: "old-0",
        previous_provider: Some("old"),
        auth: "main",
        command: &launch,
        lease: &fixture.lease,
        daemon_lock: fixture.daemon_lock.clone(),
        class: 2,
    };
    assert!(!selection.variant_room(&registry));
    assert!(selection.candidates(&registry).preferred.is_none());
    assert!(matches!(
        selection.choose(&mut registry).unwrap(),
        ShardSelection::VariantBlocked
    ));
    selection.drain_variant(&registry);
    assert!(old.draining() && spare.draining());
    // Draining/exited owners still consume the key until their capacity releases.
    old.mark_exited();
    spare.mark_exited();
    assert!(!selection.variant_room(&registry));
    old.release_capacity();
    spare.release_capacity();
    registry.refresh();
    assert!(selection.variant_room(&registry));
    registry.shards.push(matching.clone());
    assert!(Arc::ptr_eq(
        &selected(selection.choose(&mut registry).unwrap()),
        &matching
    ));
    registry.shards.clear();
    registry
        .recovered
        .push(crate::runtime::provider::RecoveredReservation {
            reservation: crate::state::RecoveryReservation {
                auth: "main".into(),
                provider: "unidentified".into(),
                process: Some(crate::state::ProcessIdentity::current().unwrap()),
                gone: false,
                unknown_capacity: false,
                unresolved_activity: false,
            },
            slots: 1,
            process_options: None,
        });
    registry.refresh();
    assert!(!selection.variant_room(&registry));
    registry.recovered[0]
        .reservation
        .process
        .as_mut()
        .unwrap()
        .birth = if cfg!(target_os = "macos") {
        "macos:0:0".into()
    } else {
        "linux:0".into()
    };
    registry.refresh();
    assert!(selection.variant_room(&registry));
    drop(pool);
    fixture.finish();
}
