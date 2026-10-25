#![allow(clippy::unwrap_used)]
use super::*;
use crate::tests::fixture;
use hive_setup_support::{
    runner::Output,
    service::{Platform, Scope},
};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

#[derive(Clone, Default)]
struct Manager {
    loaded: Rc<RefCell<BTreeSet<String>>>,
    fail_stop: Rc<Cell<bool>>,
    fail_start: Rc<Cell<bool>>,
}
impl Runner for Manager {
    fn run(&self, _: &str, args: &[String]) -> std::io::Result<Output> {
        let mut code = 0;
        let mut stdout = String::new();
        let target = args
            .get(1)
            .and_then(|a| a.rsplit('/').next())
            .unwrap_or("")
            .to_owned();
        match args[0].as_str() {
            "print-disabled" => stdout = "disabled services = {\n}\n".into(),
            "print" => {
                if !self.loaded.borrow().contains(&target) {
                    code = 113;
                }
            }
            "bootout" => {
                if self.fail_stop.get() {
                    code = 5;
                } else {
                    self.loaded.borrow_mut().remove(&target);
                }
            }
            "bootstrap" => {
                if self.fail_start.get() {
                    code = 5;
                } else {
                    self.loaded.borrow_mut().insert(
                        Path::new(&args[2])
                            .file_stem()
                            .unwrap()
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
            "enable" | "kickstart" => (),
            other => panic!("Unexpected manager call {other}"),
        }
        Ok(Output {
            code,
            stdout,
            stderr: "injected manager failure".into(),
        })
    }
}
struct World {
    temp: tempfile::TempDir,
    prefix: std::path::PathBuf,
    layout: Layout,
    manager: Manager,
    running: String,
    stopped: String,
}
mod publication;
impl World {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let prefix = home.join("prefix");
        let part = fixture(&home, "hive");
        publish::install(&prefix, &home, &[part], |_| Ok(())).unwrap();
        let layout = Layout {
            home: home.clone(),
            config: home.join("config"),
            state: home.join("state"),
            platform: Platform::Launchd,
        };
        let manager = Manager::default();
        let mut labels = vec![];
        for (name, running) in [("running", true), ("stopped", false)] {
            let scope = Scope::new(
                "hive",
                &[&home.join(format!("{name}.yaml")), &home.join(name)],
            )
            .unwrap();
            let spec = Spec::new(
                &layout,
                scope.clone(),
                &prefix.join("bin/hivectl"),
                vec![],
                None,
                vec![],
            )
            .unwrap();
            let ctl = Controller {
                layout: &layout,
                scope: &scope,
                runner: &manager,
            };
            ctl.install(&spec).unwrap();
            let label = scope.label().unwrap();
            if running {
                manager.loaded.borrow_mut().insert(label.clone());
            }
            labels.push(label);
        }
        Self {
            temp,
            prefix,
            layout,
            manager,
            running: labels.remove(0),
            stopped: labels.remove(0),
        }
    }
    fn services(&self) -> Services {
        let manager = self.manager.clone();
        Services {
            yes: true,
            runner: Box::new(manager.clone()),
            health: Box::new(move |s| {
                Ok(Health {
                    ready: manager.loaded.borrow().contains(&s.scope.label()?),
                    loaded_agents: Some(3),
                    active_turns: Some(1),
                })
            }),
            layout: self.layout.clone(),
            held: vec![],
            journal: None,
        }
    }
}
#[test]
fn crash_recovery_uses_saved_running_set_before_and_after_pointer_publication() {
    for point in ["before_replace", "after_replace"] {
        let world = World::new();
        let old = publish::current(&world.prefix.join("lib/hive")).unwrap();
        let part = fixture(world.temp.path(), "hive-core");
        let mut services = world.services();
        let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
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
        assert!(crash.is_err());
        drop(services);
        assert!(world.manager.loaded.borrow().is_empty());
        let manager = world.prefix.join("lib/hive");
        assert_eq!(
            publish::current(&manager).unwrap() == old,
            point == "before_replace"
        );
        assert!(manager.join(PENDING).is_file());
        world.services().recover(&world.prefix, &manager).unwrap();
        assert!(world.manager.loaded.borrow().contains(&world.running));
        assert!(!world.manager.loaded.borrow().contains(&world.stopped));
        assert!(!manager.join(PENDING).exists());
    }
}
#[test]
fn failed_stop_keeps_old_binaries_and_failed_restart_keeps_new_with_recovery() {
    for failure in ["stop", "restart"] {
        let world = World::new();
        let manager = world.prefix.join("lib/hive");
        let old = publish::current(&manager).unwrap();
        world.manager.fail_stop.set(failure == "stop");
        world.manager.fail_start.set(failure == "restart");
        let part = fixture(world.temp.path(), "hive-core");
        let error = publish::install_with(
            &world.prefix,
            world.temp.path(),
            &[part],
            &mut world.services(),
            |_| Ok(()),
        )
        .err()
        .unwrap()
        .to_string();
        assert_eq!(
            publish::current(&manager).unwrap() == old,
            failure == "stop"
        );
        if failure == "restart" {
            assert!(error.contains("New binaries are installed"));
            assert!(manager.join(PENDING).exists());
            world.manager.fail_start.set(false);
            world.services().recover(&world.prefix, &manager).unwrap();
        }
        assert!(world.manager.loaded.borrow().contains(&world.running));
        assert!(!world.manager.loaded.borrow().contains(&world.stopped));
        assert!(!manager.join(PENDING).exists());
    }
}

#[test]
fn inventory_ignores_failure_records_but_refuses_mislabeled_descriptors() {
    let world = World::new();
    let components = vec!["hive".into()];
    let specs = inventory(&world.layout, &world.prefix, &components).unwrap();
    files::write(
        &specs[0].descriptor.with_extension("failure.json"),
        b"{\"code\":\"config_invalid\"}",
        false,
    )
    .unwrap();
    assert_eq!(
        inventory(&world.layout, &world.prefix, &components)
            .unwrap()
            .len(),
        2
    );
    files::write(
        &world.layout.config.join("hive/services/wrong.json"),
        &files::encode(&specs[0]).unwrap(),
        false,
    )
    .unwrap();
    assert!(inventory(&world.layout, &world.prefix, &components).is_err());
}

#[test]
fn prepare_probe_failure_discards_only_its_unpublished_generation() {
    let world = World::new();
    let manager = world.prefix.join("lib/hive");
    let before = fs::read_dir(manager.join("generations")).unwrap().count();
    let pointer = publish::current(&manager).unwrap();
    let mut services = world.services();
    services.health = Box::new(|_| Err("injected probe error".into()));
    let part = fixture(world.temp.path(), "hive-core");
    assert!(
        publish::install_with(
            &world.prefix,
            world.temp.path(),
            &[part],
            &mut services,
            |_| Ok(())
        )
        .is_err()
    );
    assert_eq!(
        before,
        fs::read_dir(manager.join("generations")).unwrap().count()
    );
    assert_eq!(pointer, publish::current(&manager).unwrap());
    assert!(world.manager.loaded.borrow().contains(&world.running));
    assert!(!manager.join(PENDING).exists());
}

#[test]
fn renderer_upgrade_and_crash_recovery_choose_unit_for_actual_pointer() {
    for point in ["before_replace", "after_replace", "success"] {
        let world = World::new();
        let old_specs = inventory(&world.layout, &world.prefix, &["hive".into()])
            .unwrap()
            .into_iter()
            .map(|mut spec| {
                spec.unit_contents.push_str("\n<!-- former renderer -->\n");
                files::write(&spec.unit, spec.unit_contents.as_bytes(), true).unwrap();
                files::write(&spec.descriptor, &files::encode(&spec).unwrap(), true).unwrap();
                spec
            })
            .collect::<Vec<_>>();
        let part = fixture(world.temp.path(), "hive-core");
        let mut services = world.services();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            publish::install_with(
                &world.prefix,
                world.temp.path(),
                &[part],
                &mut services,
                |at| {
                    assert_ne!(at, point, "crash");
                    Ok(())
                },
            )
        }));
        if point == "success" {
            assert!(result.unwrap().is_ok());
        } else {
            assert!(result.is_err());
        }
        drop(services);
        world
            .services()
            .recover(&world.prefix, &world.prefix.join("lib/hive"))
            .unwrap();
        for old in old_specs {
            let expected = if point == "before_replace" {
                old.unit_contents.clone()
            } else {
                old.rendered().unwrap().unit_contents
            };
            assert_eq!(
                files::read(&old.unit).unwrap().unwrap(),
                expected.as_bytes()
            );
        }
        assert!(world.manager.loaded.borrow().contains(&world.running));
        assert!(!world.manager.loaded.borrow().contains(&world.stopped));
    }
}

#[test]
fn uninstall_crashes_restore_before_commit_and_finish_removal_after_commit() {
    for point in ["before_replace", "after_replace", "after_links"] {
        let world = World::new();
        let manager = world.prefix.join("lib/hive");
        let old = publish::current(&manager).unwrap();
        let specs = inventory(&world.layout, &world.prefix, &["hive".into()]).unwrap();
        let mut services = world.services();
        let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::uninstall::remove_with(
                &world.prefix,
                &["hive".into()],
                true,
                false,
                &mut services,
                |at| {
                    assert_ne!(at, point, "crash");
                    Ok(())
                },
            )
        }));
        assert!(crash.is_err());
        drop(services);
        assert!(specs.iter().all(|s| !s.descriptor.exists()));
        world.services().recover(&world.prefix, &manager).unwrap();
        let before = point == "before_replace";
        assert_eq!(publish::current(&manager).unwrap() == old, before);
        assert_eq!(world.prefix.join("bin/hivectl").is_file(), before);
        assert_eq!(specs.iter().all(|s| s.descriptor.exists()), before);
        assert_eq!(
            world.manager.loaded.borrow().contains(&world.running),
            before
        );
        assert!(!world.manager.loaded.borrow().contains(&world.stopped));
        assert!(!manager.join(PENDING).exists());
    }
}

#[test]
fn uninstall_keeps_original_native_error_when_restore_fails_or_commit_is_incomplete() {
    for point in ["before_replace", "after_replace"] {
        let world = World::new();
        let manager = world.prefix.join("lib/hive");
        let old = publish::current(&manager).unwrap();
        world.manager.fail_start.set(true);
        let mut services = world.services();
        let path = world.temp.path().join("uninstall-log.jsonl");
        let (backend, guard) = hive_log::start(&path, hive_log::Config::default()).unwrap();
        let error = backend
            .scope(|| {
                crate::uninstall::remove_with(
                    &world.prefix,
                    &["hive".into()],
                    true,
                    false,
                    &mut services,
                    |at| {
                        if at == point {
                            Err(std::io::Error::from_raw_os_error(
                                rustix::io::Errno::NOSPC.raw_os_error(),
                            )
                            .into())
                        } else {
                            Ok(())
                        }
                    },
                )
            })
            .err()
            .unwrap();
        assert_eq!(
            guard.finish(std::time::Duration::from_secs(1)).state,
            hive_log::Shutdown::Drained
        );
        let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error.as_ref());
        let mut retained = false;
        while let Some(cause) = source {
            retained |= cause.downcast_ref::<std::io::Error>().is_some_and(|cause| {
                cause.raw_os_error() == Some(rustix::io::Errno::NOSPC.raw_os_error())
            });
            source = cause.source();
        }
        assert!(retained, "original error chain disappeared");
        assert!(manager.join(PENDING).exists());
        let log = std::fs::read_to_string(path).unwrap();
        if point == "before_replace" {
            assert!(log.contains("install.cleanup_failed"));
        } else {
            assert!(log.contains("install.uninstall_pending"));
        }
        drop(services);
        world.manager.fail_start.set(false);
        world.services().recover(&world.prefix, &manager).unwrap();
        assert_eq!(
            publish::current(&manager).unwrap() == old,
            point == "before_replace"
        );
    }
}

#[test]
fn uninstall_preserves_other_component_and_native_data_and_can_repeat() {
    let world = World::new();
    let comb = fixture(world.temp.path(), "comb");
    publish::install(&world.prefix, world.temp.path(), &[comb], |_| Ok(())).unwrap();
    let config = world.temp.path().join("config.yaml");
    fs::write(&config, "retained").unwrap();
    let components = vec!["hive".into()];
    let removed = crate::uninstall::remove_with(
        &world.prefix,
        &components,
        true,
        false,
        &mut world.services(),
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(removed.active, 1);
    assert!(removed.binaries["hive"].is_file());
    assert!(!world.prefix.join("bin/hivectl").exists());
    assert!(world.prefix.join("bin/comb").is_file());
    assert!(world.prefix.join("bin/hive-install").is_file());
    assert_eq!(fs::read_to_string(config).unwrap(), "retained");
    let repeat = crate::uninstall::remove_with(
        &world.prefix,
        &components,
        true,
        false,
        &mut world.services(),
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(repeat.binaries, removed.binaries);
    assert_eq!(repeat.active, 1);
    let manager = world.prefix.join("lib/hive");
    let image = publish::load(
        &manager,
        publish::current(&manager).unwrap().as_deref(),
        &crate::manifest::native_target().unwrap(),
    )
    .unwrap();
    assert!(image.retired.contains_key("hive"));
    assert!(!image.parts.contains_key("hive"));
}

#[test]
fn uninstall_failed_stop_preserves_publication_and_active_services() {
    let world = World::new();
    let manager = world.prefix.join("lib/hive");
    let pointer = publish::current(&manager).unwrap();
    world.manager.fail_stop.set(true);
    assert!(
        crate::uninstall::remove_with(
            &world.prefix,
            &["hive".into()],
            true,
            false,
            &mut world.services(),
            |_| Ok(())
        )
        .is_err()
    );
    assert_eq!(publish::current(&manager).unwrap(), pointer);
    assert!(world.prefix.join("bin/hivectl").is_file());
    assert!(world.manager.loaded.borrow().contains(&world.running));
    assert!(!manager.join(PENDING).exists());
}

#[test]
fn standalone_uninstall_leaves_hive_services_running_and_reinstall_clears_retirement() {
    let world = World::new();
    let comb = fixture(world.temp.path(), "comb");
    publish::install(
        &world.prefix,
        world.temp.path(),
        std::slice::from_ref(&comb),
        |_| Ok(()),
    )
    .unwrap();
    let removed = crate::uninstall::remove_with(
        &world.prefix,
        &["comb".into()],
        true,
        false,
        &mut world.services(),
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(removed.active, 1);
    assert!(world.prefix.join("bin/hivectl").is_file());
    assert!(!world.prefix.join("bin/comb").exists());
    assert!(world.manager.loaded.borrow().contains(&world.running));
    publish::install(&world.prefix, world.temp.path(), &[comb], |_| Ok(())).unwrap();
    let manager = world.prefix.join("lib/hive");
    let installed = publish::load(
        &manager,
        publish::current(&manager).unwrap().as_deref(),
        &crate::manifest::native_target().unwrap(),
    )
    .unwrap();
    assert!(installed.retired.is_empty());
    assert!(world.prefix.join("bin/comb").is_file());
}

#[test]
fn public_removal_path_recovers_half_removed_unit_before_strict_preview() {
    let world = World::new();
    let manager = world.prefix.join("lib/hive");
    let old = publish::current(&manager).unwrap().unwrap();
    let specs = inventory(&world.layout, &world.prefix, &["hive".into()]).unwrap();
    let installed = publish::load(
        &manager,
        Some(&old),
        &crate::manifest::native_target().unwrap(),
    )
    .unwrap();
    // Durable admission happened, then a crash removed only one stopped unit.
    let change = Change {
        operation: Operation::Uninstall,
        prefix: world.prefix.clone(),
        manager: manager.clone(),
        previous: Some(old.clone()),
        next: "generations/g-unpublished-test".into(),
        components: vec!["hive".into()],
        added_links: BTreeMap::new(),
        removed_links: publish::links(&installed.parts).unwrap(),
    };
    let mut admission = world.services();
    admission.prepare(&change).unwrap();
    world.manager.loaded.borrow_mut().clear();
    fs::remove_file(&specs[0].unit).unwrap();
    drop(admission);
    assert!(inventory(&world.layout, &world.prefix, &["hive".into()]).is_err());
    // Exercise public run's common path, not recover() in isolation.
    let result = crate::uninstall::remove_with(
        &world.prefix,
        &["hive".into()],
        true,
        false,
        &mut world.services(),
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(result.active, 0);
    assert!(!world.prefix.join("bin/hivectl").exists());
    assert!(!manager.join(PENDING).exists());
}

#[test]
fn new_service_registration_is_fenced_after_uninstall_inventory() {
    let world = World::new();
    let scope = Scope::new(
        "hive",
        &[
            &world.temp.path().join("new.yaml"),
            &world.temp.path().join("new"),
        ],
    )
    .unwrap();
    let spec = Spec::new(
        &world.layout,
        scope.clone(),
        &world.prefix.join("bin/hivectl"),
        vec![],
        None,
        vec![],
    )
    .unwrap();
    let ctl = Controller {
        layout: &world.layout,
        scope: &scope,
        runner: &world.manager,
    };
    let mut attempted = false;
    crate::uninstall::remove_with(
        &world.prefix,
        &["hive".into()],
        true,
        false,
        &mut world.services(),
        |point| {
            if point == "before_replace" {
                attempted = true;
                let error = ctl.install_new(&spec).unwrap_err();
                assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
                assert!(std::error::Error::source(&error).is_some());
                assert!(!spec.descriptor.exists());
            }
            Ok(())
        },
    )
    .unwrap();
    assert!(attempted);
    assert!(ctl.install(&spec).is_err());
    assert!(!spec.descriptor.exists());
}
