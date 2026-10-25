use super::*;

fn interrupt_addition(world: &World, point: &str) {
    let part = fixture(world.temp.path(), "comb");
    let mut services = world.services();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        publish::install_with(
            &world.prefix,
            world.temp.path(),
            &[part],
            &mut services,
            |at| {
                assert_ne!(at, point, "simulated process loss");
                Ok(())
            },
        )
    }));
    assert!(result.is_err());
}

#[test]
fn selected_generation_recovers_missing_added_links_including_partial_completion() {
    for point in ["after_replace", "after_sync", "link_created", "after_links"] {
        let world = World::new();
        interrupt_addition(&world, point);
        let manager = world.prefix.join("lib/hive");
        assert!(manager.join(PENDING).exists());
        if matches!(point, "after_replace" | "after_sync") {
            assert!(!world.prefix.join("bin/comb").exists());
            assert!(!world.prefix.join("share/comb").exists());
        }
        world.services().recover(&world.prefix, &manager).unwrap();
        for name in ["bin/comb", "share/comb", "bin/hivectl"] {
            assert!(world.prefix.join(name).exists(), "{point}: {name}");
        }
        assert!(!manager.join(PENDING).exists());
        world.services().recover(&world.prefix, &manager).unwrap();
    }
}

#[test]
fn foreign_added_link_survives_recovery_and_journal_is_retained() {
    let world = World::new();
    interrupt_addition(&world, "after_replace");
    let path = world.prefix.join("bin/comb");
    fs::write(&path, b"foreign").unwrap();
    let manager = world.prefix.join("lib/hive");
    let pointer = publish::current(&manager).unwrap();
    assert!(world.services().recover(&world.prefix, &manager).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"foreign");
    assert!(manager.join(PENDING).exists());
    assert_eq!(publish::current(&manager).unwrap(), pointer);
    // It also must not create other links before detecting the foreign entry.
    assert!(!world.prefix.join("share/comb").exists());
}

#[test]
fn incomplete_journal_authority_never_repairs_an_added_link() {
    let world = World::new();
    interrupt_addition(&world, "after_replace");
    let manager = world.prefix.join("lib/hive");
    let path = manager.join(PENDING);
    let mut journal: Journal = files::json(&path).unwrap().unwrap();
    journal.change.added_links.clear();
    files::write(&path, &files::encode(&journal).unwrap(), true).unwrap();
    assert!(world.services().recover(&world.prefix, &manager).is_err());
    assert!(path.exists());
    assert!(!world.prefix.join("bin/comb").exists());
}
