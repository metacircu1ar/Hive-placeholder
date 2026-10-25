use super::*;
use crate::{cli::response_tests::Stub, error::Error};
use serde_json::json;
use std::{cell::Cell, error::Error as _, fs};

fn fixture(root: &Path) -> (Expected, serde_json::Value) {
    let config = root.join("config.yaml");
    fs::write(&config, "auth_profiles: {}\n").unwrap();
    let expected = expectation(&config, "owned".into()).unwrap();
    let value = json!({"binary_sha256":"d".repeat(64),"protocols":{"minimum":1,"maximum":1},"configuration":expected.configuration,"provider_selection":{
        "codex":"a".repeat(64),"claude":"b".repeat(64)},"status":"running","managed_service":"owned"});
    (expected, value)
}

#[test]
fn daemon_start_managed_probes_preserve_service_selection_and_refuse_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let (expected, value) = fixture(tmp.path());
    let stub = Stub::new(vec![value.clone()]);
    assert!(
        checked(&expected, stub.root(), |health| {
            assert!(health()?);
            Ok(())
        })
        .unwrap()
        .is_none()
    );
    stub.finish();
    for field in ["config", "owner"] {
        let mut changed = value.clone();
        if field == "config" {
            changed["configuration"]["sha256"] = json!("c".repeat(64));
        } else {
            changed["managed_service"] = json!("other");
        }
        let stub = Stub::new(vec![changed]);
        let starts = Cell::new(0);
        let error = checked(&expected, stub.root(), |health| {
            if !health()? {
                starts.set(starts.get() + 1);
            }
            Ok(())
        })
        .err()
        .unwrap();
        if field == "config" {
            let Error::Input { input, .. } = error else {
                panic!("typed input")
            };
            assert_eq!(input.field(), "--config");
        } else {
            assert_eq!(error.code(), "config_invalid");
        }
        assert_eq!(starts.get(), 0);
        stub.finish();
    }
}

#[test]
fn daemon_start_managed_callback_retains_native_and_wire_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let (expected, _) = fixture(tmp.path());
    let error = checked(&expected, &tmp.path().join("config.yaml"), |health| {
        health()?;
        Ok(())
    })
    .err()
    .unwrap();
    assert!(
        error
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .is_some()
    );
    let wire = json!({"error":{"code":"daemon_unavailable","message":"wire failure"}});
    let stub = Stub::new(vec![wire.clone()]);
    let outcome = checked(&expected, stub.root(), |health| {
        health()?;
        Ok(())
    })
    .unwrap()
    .unwrap();
    assert!(!outcome.success);
    assert_eq!(outcome.value, wire);
    stub.finish();
    let starts = Cell::new(0);
    assert!(
        checked(&expected, &tmp.path().join("missing"), |health| {
            assert!(!health()?);
            starts.set(1);
            Ok(())
        })
        .unwrap()
        .is_none()
    );
    assert_eq!(starts.get(), 1);
}

#[test]
fn daemon_start_managed_overrides_are_rejected_including_explicit_defaults() {
    use clap::Parser;
    for flag in ["--codex-bin", "--claude-bin"] {
        let mut cli = crate::cli::Cli::try_parse_from(["hivectl", "daemon", "start"]).unwrap();
        assert!(overrides(&cli).is_ok());
        cli.explicit_provider = Some(flag);
        let Error::Input { input, .. } = overrides(&cli).unwrap_err() else {
            panic!("input error")
        };
        assert_eq!(input.field(), flag);
    }
}

#[test]
fn daemon_start_managed_readiness_is_checked_on_every_callback() {
    let tmp = tempfile::tempdir().unwrap();
    let (expected, value) = fixture(tmp.path());
    let mut changed = value.clone();
    changed["configuration"]["sha256"] = json!("e".repeat(64));
    let stub = Stub::new(vec![value, changed]);
    let error = checked(&expected, stub.root(), |health| {
        assert!(health()?);
        // A later observation must not reuse the first successful snapshot.
        health()?;
        Ok(())
    })
    .err()
    .unwrap();
    assert_eq!(error.code(), "conflicting_arguments");
    assert_eq!(stub.finish().len(), 2);
}

#[test]
fn managed_restart_checks_original_owner_before_any_manager_start() {
    use hive_setup_support::{
        runner::{Output, Runner},
        service::{Controller, Layout, Platform, Scope, Spec},
    };
    struct Manager(Cell<bool>, Cell<usize>);
    impl Runner for Manager {
        fn run(&self, _: &str, args: &[String]) -> std::io::Result<Output> {
            let mut code = 0;
            match args[0].as_str() {
                "print-disabled" | "enable" => (),
                "print" => {
                    if !self.0.get() {
                        code = 113;
                    }
                }
                "bootstrap" | "kickstart" => {
                    self.0.set(true);
                    self.1.set(self.1.get() + 1);
                }
                "bootout" => self.0.set(false),
                unknown => panic!("Unexpected manager call: {unknown}"),
            }
            Ok(Output {
                code,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }
    let temp = crate::runtime::test_directory();
    let home = temp.path().canonicalize().unwrap();
    let root = home.join("hive-state");
    let state = crate::state::State::new(root.clone()).unwrap();
    let initialization = state.initialize().unwrap();
    drop(initialization);
    let owner = crate::runtime::purge_lock(&root).unwrap();
    assert!(!crate::runtime::socket_path(&root).exists());
    let layout = Layout {
        config: home.join("config"),
        state: home.join("services"),
        home: home.clone(),
        platform: Platform::Launchd,
    };
    let scope = Scope::new("hive", &[&home.join("hive.yaml"), &root]).unwrap();
    let spec = Spec::new(
        &layout,
        scope.clone(),
        Path::new("/bin/echo"),
        vec!["fixture".into()],
        None,
        vec![],
    )
    .unwrap();
    let manager = Manager(Cell::new(false), Cell::new(0));
    let controller = Controller {
        layout: &layout,
        scope: &scope,
        runner: &manager,
    };
    controller.install_new(&spec).unwrap();
    let held = controller.hold().unwrap();
    let restart = || {
        super::super::restart_held(
            &held,
            &manager,
            &root,
            Instant::now(),
            &|| Ok(manager.0.get()),
            || {
                held.start(&manager, &|| Ok(manager.0.get()))
                    .map_err(super::super::error)
            },
        )
    };
    assert_eq!(restart().unwrap_err().code(), "daemon_start_timeout");
    assert_eq!(manager.1.get(), 0);
    drop(owner);
    restart().unwrap();
    assert_eq!(manager.1.get(), 1);
}
