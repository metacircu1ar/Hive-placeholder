use super::*;

#[test]
fn payload_replacements_at_rename_and_unlink_boundaries_are_not_deleted() {
    let _fork = crate::os::fork_guard().unwrap();
    for before_rename in [true, false] {
        let root = private_root().unwrap();
        let image = fixture(root.path());
        let original = image.directory.clone();
        let aside = image.retirement.aside.clone();
        let saved = root.path().join("saved");
        let retained = saved.clone();
        let once = Cell::new(false);
        let result = state::sync_tests::with_hook(
            move |path, step| {
                let trigger = if before_rename {
                    path == original && step == Step::BeforePublish
                } else {
                    path == aside.join("hive-launch") && step == Step::BeforeRemove
                };
                if trigger && !once.replace(true) {
                    let payload = if before_rename {
                        original.join("hive-launch")
                    } else {
                        aside.join("hive-launch")
                    };
                    fs::rename(&payload, &retained)?;
                    state::private_file(&payload, true)
                        .unwrap()
                        .write_all(b"replacement sentinel")?;
                }
                Ok(())
            },
            || image.cleanup(),
        );
        assert!(result.is_err());
        let retirement = abandon(image);
        assert_eq!(
            fs::read(retirement.aside.join("hive-launch")).unwrap(),
            b"replacement sentinel"
        );
        assert_eq!(fs::read(saved).unwrap(), b"launcher");
    }
}

#[test]
fn markerless_retry_removes_only_the_original_empty_directory() {
    let _fork = crate::os::fork_guard().unwrap();
    for defect in ["empty", "known", "unknown", "replacement"] {
        let root = private_root().unwrap();
        let retirement = retire(root.path());
        for entry in fs::read_dir(&retirement.aside).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        match defect {
            "known" => {
                state::private_file(&retirement.aside.join("hive-launch"), true).unwrap();
            }
            "unknown" => {
                state::private_file(&retirement.aside.join("sentinel"), true).unwrap();
            }
            "replacement" => {
                fs::rename(&retirement.aside, root.path().join("saved")).unwrap();
                state::directory(&retirement.aside).unwrap();
            }
            _ => {}
        }
        sweep(root.path());
        assert_eq!(retirement.aside.exists(), defect != "empty", "{defect}");
    }
}

#[test]
fn collisions_and_unproven_ownership_retain_every_entry() {
    let _fork = crate::os::fork_guard().unwrap();
    for defect in [
        "both",
        "duplicate",
        "unknown",
        "public",
        "held",
        "symlink",
        "hardlink",
    ] {
        let root = private_root().unwrap();
        let retirement = retire(root.path());
        let name = retirement.original.file_name().unwrap().to_str().unwrap();
        let mut owner = None;
        match defect {
            "both" => state::directory(&retirement.original).unwrap(),
            "duplicate" => {
                state::directory(&root.path().join(format!(".hive-retiring-{name}.invalid")))
                    .unwrap();
            }
            "unknown" => {
                state::private_file(&retirement.aside.join("sentinel"), true).unwrap();
            }
            "public" => fs::set_permissions(
                retirement.aside.join("hive-launch"),
                fs::Permissions::from_mode(0o644),
            )
            .unwrap(),
            "held" => {
                owner = Some(
                    state::Lock::acquire_existing(&retirement.aside.join("owner.lock")).unwrap(),
                );
            }
            "symlink" | "hardlink" => {
                let path = retirement.aside.join("hive-launch");
                fs::rename(&path, root.path().join("saved")).unwrap();
                if defect == "symlink" {
                    std::os::unix::fs::symlink(root.path().join("saved"), path).unwrap();
                } else {
                    fs::hard_link(root.path().join("saved"), path).unwrap();
                }
            }
            _ => unreachable!(),
        }
        sweep(root.path());
        assert!(
            retirement
                .aside
                .join("hive-launch")
                .symlink_metadata()
                .is_ok(),
            "{defect}"
        );
        assert!(retirement.aside.join("owner.lock").exists(), "{defect}");
        drop(owner);
    }
}

#[test]
fn identity_names_are_canonical_and_allocations_refuse_retained_asides() {
    let _fork = crate::os::fork_guard().unwrap();
    let root = private_root().unwrap();
    let retirement = retire(root.path());
    let parsed = Retirement::parse(&retirement.aside).unwrap();
    assert_eq!(parsed.original, retirement.original);
    assert_eq!(parsed.parent, retirement.parent);
    assert_eq!(parsed.tree, retirement.tree);
    for parent in [true, false] {
        let mut bad = parsed.clone();
        if parent {
            bad.parent.inode += 1;
        } else {
            bad.tree.inode += 1;
        }
        assert!(bad.remove(None).is_err());
        assert!(retirement.aside.join("hive-launch").exists());
    }
    assert!(
        Retirement::allocate(
            root.path(),
            retirement.original.file_name().unwrap().to_str().unwrap()
        )
        .is_err()
    );
    for name in [
        ".hive-retiring-../bad",
        ".hive-retiring-hive-launch-x.00",
        ".hive-retiring-hive-launch-.0000000000000000000000000000000000000000000000000000000000000000",
    ] {
        assert!(Retirement::parse(&root.path().join(name)).is_err());
    }
    let original = Retirement::allocate(root.path(), "hive-launch-no-owner").unwrap();
    sweep(root.path());
    assert!(original.original.exists());
    original.remove_empty_reservation().unwrap();
}

#[test]
fn malformed_aside_is_retained_and_blocks_reallocation() {
    let _fork = crate::os::fork_guard().unwrap();
    let root = private_root().unwrap();
    let retirement = retire(root.path());
    let name = retirement.original.file_name().unwrap().to_str().unwrap();
    let bad = root.path().join(format!(".hive-retiring-{name}.invalid"));
    fs::rename(&retirement.aside, &bad).unwrap();
    assert!(Retirement::parse(&bad).is_err());
    assert!(Retirement::allocate(root.path(), name).is_err());
    sweep(root.path());
    assert!(bad.join("hive-launch").exists());
}

#[test]
fn an_aside_named_before_a_remount_is_matched_by_its_inodes() {
    let _fork = crate::os::fork_guard().unwrap();
    let root = private_root().unwrap();
    let retirement = retire(root.path());
    let name = retirement.original.file_name().unwrap().to_str().unwrap();
    let (device, parent, tree) = (
        retirement.parent.device,
        retirement.parent.inode,
        retirement.tree.inode,
    );
    let named = |parent_device: u64, parent_inode: u64, tree_device: u64, tree_inode: u64| {
        root.path().join(format!(
            ".hive-retiring-{name}.{parent_device:016x}{parent_inode:016x}{tree_device:016x}{tree_inode:016x}"
        ))
    };
    let renumbered = device.wrapping_add(1);
    let mut current = retirement.aside.clone();
    // Disagreeing devices or another directory's inode keep the aside.
    for aside in [
        named(renumbered, parent, renumbered.wrapping_add(1), tree),
        named(renumbered, parent.wrapping_add(1), renumbered, tree),
        named(renumbered, parent, renumbered, tree.wrapping_add(1)),
    ] {
        fs::rename(&current, &aside).unwrap();
        current = aside;
        sweep(root.path());
        assert!(
            current.join("hive-launch").exists(),
            "{}",
            current.display()
        );
    }
    // A remount renumbered the device; the same inodes are still removed.
    let aside = named(renumbered, parent, renumbered, tree);
    fs::rename(&current, &aside).unwrap();
    sweep(root.path());
    assert!(!aside.exists());
    assert!(
        fs::read_dir(root.path())
            .unwrap()
            .all(|entry| { !is_aside(entry.unwrap().file_name().to_str().unwrap()) })
    );
}
