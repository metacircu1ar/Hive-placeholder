//! Services regressions.
use super::*;

#[test]
fn lifecycle_is_idempotent_owned_and_restores_only_running_services() {
    let (_temp, layout, scope, spec) = fixture();
    let mock = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    let health = || Ok(mock.ready.get());
    ctl.install(&spec).unwrap();
    let bytes = files::read(&spec.unit).unwrap();
    ctl.install(&spec).unwrap();
    assert_eq!(files::read(&spec.unit).unwrap(), bytes);
    ctl.start(&health).unwrap();
    assert!(ctl.status(&health).unwrap().ready);
    let calls = mock
        .calls
        .borrow()
        .iter()
        .filter(|c| matches!(c[0].as_str(), "bootstrap" | "kickstart"))
        .count();
    ctl.start(&health).unwrap();
    assert_eq!(
        calls,
        mock.calls
            .borrow()
            .iter()
            .filter(|c| matches!(c[0].as_str(), "bootstrap" | "kickstart"))
            .count()
    );
    let result: crate::Result<()> = ctl.paused(&health, || {
        assert!(!mock.ready.get());
        Err(crate::invalid("activation failed"))
    });
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("activation failed")
    );
    assert!(mock.ready.get());
    ctl.stop(&health).unwrap();
    ctl.paused(&health, || Ok(())).unwrap();
    assert!(!mock.ready.get());
    ctl.uninstall(&health).unwrap();
    ctl.uninstall(&health).unwrap();
    assert!(!spec.unit.exists());
    assert!(!spec.descriptor.exists());
}

#[test]
fn interrupted_install_completes_exact_owned_descriptor_only() {
    let (_temp, layout, scope, spec) = fixture();
    let mock = Mock::new();
    files::write(&spec.descriptor, &files::encode(&spec).unwrap(), false).unwrap();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    ctl.install(&spec).unwrap();
    assert!(service::load(&layout, &scope).unwrap().is_some());
}

#[test]
fn unavailable_manager_is_visible_and_installs_nothing() {
    let (_temp, layout, scope, spec) = fixture();
    let mock = Mock::new();
    mock.unavailable.set(true);
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    let status = ctl.status(&|| Ok(false)).unwrap();
    assert!(!status.manager_available);
    assert!(!status.managed);
    assert!(ctl.install(&spec).is_err());
    assert!(!spec.descriptor.exists());
    assert!(!spec.unit.exists());
}

#[test]
fn environment_is_private_literal_and_not_copied_into_units() {
    let (_temp, layout, scope, mut spec) = fixture();
    let path = layout.home.join("secret.env");
    files::write(&path, b"API_KEY=literal $(do-not-run) $HOME\n", false).unwrap();
    assert_eq!(
        environment::read(Some(&path)).unwrap()["API_KEY"],
        "literal $(do-not-run) $HOME"
    );
    spec.environment_file = Some(path.clone());
    spec.required_environment = vec!["API_KEY".into()];
    spec.check_environment().unwrap();
    assert!(
        !String::from_utf8(spec.render().unwrap())
            .unwrap()
            .contains("do-not-run")
    );
    for bad in [
        "A=1\nA=2",
        "bad-key=secret",
        "X=bad\0value",
        "HOME=foreign",
        "export A=1",
    ] {
        files::write(&path, bad.as_bytes(), true).unwrap();
        assert!(environment::read(Some(&path)).is_err());
    }
    files::write(&path, b"OTHER=value", true).unwrap();
    let mock = Mock::new();
    assert!(
        Controller {
            layout: &layout,
            scope: &scope,
            runner: &mock
        }
        .install(&spec)
        .is_err()
    );
    assert!(!spec.unit.exists());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(environment::read(Some(&path)).is_err());
}

#[test]
fn controller_keeps_waiting_after_transient_startup_failure() {
    let (_temp, layout, scope, spec) = fixture();
    let mock = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    ctl.install(&spec).unwrap();
    let calls = Cell::new(0);
    ctl.start(&|| {
        let n = calls.get();
        calls.set(n + 1);
        if n == 1 {
            files::write(
                &spec.descriptor.with_extension("failure.json"),
                &files::encode(&crate::managed::Failure {
                    code: "io_error".into(),
                    suppressed: false,
                })?,
                true,
            )?;
        }
        Ok(n >= 2)
    })
    .unwrap();
    assert_eq!(calls.get(), 3);
}

#[test]
fn interrupted_uninstall_only_finishes_after_confirmed_stop() {
    let (_temp, layout, scope, spec) = fixture();
    let mock = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    ctl.install(&spec).unwrap();
    files::remove(&spec.unit, &spec.render().unwrap()).unwrap();
    mock.loaded.set(true);
    assert!(ctl.uninstall(&|| Ok(false)).is_err());
    assert!(spec.descriptor.exists());
    mock.loaded.set(false);
    ctl.uninstall(&|| Ok(false)).unwrap();
    assert!(!spec.descriptor.exists());
}

#[test]
fn explicit_service_path_rejects_invalid_entries_and_deduplicates_toolchains() {
    let (_temp, layout, _, mut spec) = fixture();
    let path = format!(
        "{}:/usr/bin:{}",
        layout.home.display(),
        layout.home.display()
    );
    let captured = environment::service_path(Some(&path)).unwrap();
    for invalid in [
        "",
        ".",
        "relative",
        "/missing-toolchain",
        "/usr/bin:",
        "/usr/bin:.",
        "/usr/bin:/missing-toolchain",
        "/usr/bin:\n",
    ] {
        assert!(
            environment::service_path(Some(invalid)).is_err(),
            "{invalid:?}"
        );
    }
    assert_eq!(captured, format!("{}:/usr/bin", layout.home.display()));
    spec.environment.insert("PATH".into(), captured.clone());
    assert!(
        String::from_utf8(spec.render().unwrap())
            .unwrap()
            .contains(&format!("<key>PATH</key><string>{captured}</string>"))
    );
    let program = layout.home.join("provider");
    fs::write(&program, "#!/usr/bin/env custom-interpreter\n").unwrap();
    assert!(
        environment::interpreter_warning(&program, &captured)
            .unwrap()
            .is_some()
    );
    let interpreter = layout.home.join("custom-interpreter");
    fs::write(&interpreter, "#!/bin/sh\n").unwrap();
    fs::set_permissions(interpreter, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        environment::interpreter_warning(&program, &captured)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        runner::capture(
            std::process::Command::spawn,
            "/bin/sh",
            &["-c".into(), "echo cause >&2; exit 5".into()],
            Duration::from_secs(1)
        )
        .unwrap()
        .stderr
        .trim(),
        "cause"
    );
}

#[test]
fn retained_installer_guard_blocks_other_mutations_and_restores_exact_service() {
    let (_temp, layout, scope, spec) = fixture();
    let runner = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &runner,
    };
    let health = || Ok(runner.ready.get());
    ctl.install(&spec).unwrap();
    ctl.start(&health).unwrap();
    let held = ctl.hold().unwrap();
    assert!(held.loaded(&runner).unwrap());
    assert!(ctl.stop(&health).is_err());
    held.stop(&runner, &health).unwrap();
    assert!(ctl.start(&health).is_err());
    held.start(&runner, &health).unwrap();
    held.uninstall(&runner, &health).unwrap();
    assert!(!spec.descriptor.exists());
    assert!(ctl.install(&spec).is_err());
    held.install(&runner).unwrap();
    held.start(&runner, &health).unwrap();
    drop(held);
    ctl.uninstall(&health).unwrap();
    let recovery = ctl.hold_expected(&spec).unwrap();
    recovery.install(&runner).unwrap();
    recovery.start(&runner, &health).unwrap();
    assert!(runner.ready.get());
    drop(recovery);
    ctl.uninstall(&health).unwrap();
}

#[test]
fn installer_setup_preserves_stopped_but_starts_new_scopes_and_refuses_uncertainty() {
    use service::StartDecision;
    let (_temp, layout, scope, spec) = fixture();
    let runner = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &runner,
    };
    ctl.install(&spec).unwrap();
    let health = || Ok(runner.ready.get());
    assert_eq!(
        ctl.start_for_setup(&health, true).unwrap(),
        StartDecision::PreservedStopped
    );
    assert!(!runner.loaded.get());
    // The caller passes false for a just-installed scope.
    assert_eq!(
        ctl.start_for_setup(&health, false).unwrap(),
        StartDecision::Started
    );
    assert_eq!(
        ctl.start_for_setup(&health, true).unwrap(),
        StartDecision::AlreadyLoaded
    );
    ctl.stop(&health).unwrap();
    runner.unavailable.set(true);
    assert!(ctl.start_for_setup(&health, true).is_err());
    runner.unavailable.set(false);
    assert!(ctl.start_for_setup(&|| Ok(true), true).is_err());
    files::write(
        &spec.descriptor.with_extension("failure.json"),
        b"{\"code\":\"config_invalid\"}",
        false,
    )
    .unwrap();
    assert!(ctl.start_for_setup(&health, true).is_err());
    assert!(!runner.loaded.get());
}

#[test]
fn unit_refresh_recovers_between_unit_and_descriptor_publication() {
    #[derive(serde::Serialize)]
    struct RefreshFixture<'a> {
        old: &'a Spec,
        next: &'a Spec,
    }
    let (_temp, layout, scope, spec) = fixture();
    let runner = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &runner,
    };
    ctl.install(&spec).unwrap();
    let mut old = spec.clone();
    old.unit_contents.push_str("\n<!-- old -->\n");
    files::write(&old.descriptor, &files::encode(&old).unwrap(), true).unwrap();
    // Descriptor is old, unit already new: exact journal authorizes recovery.
    files::write(
        &spec.descriptor.with_extension("refresh.json"),
        &files::encode(&RefreshFixture {
            old: &old,
            next: &spec,
        })
        .unwrap(),
        false,
    )
    .unwrap();
    ctl.install(&spec).unwrap();
    spec.verify(&layout, &scope).unwrap();
    assert!(!spec.descriptor.with_extension("refresh.json").exists());
}

#[test]
fn setup_refuses_a_service_created_after_its_absence_observation() {
    let (_temp, layout, scope, spec) = fixture();
    let runner = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &runner,
    };
    assert!(!ctl.status(&|| Ok(false)).unwrap().managed);
    ctl.install(&spec).unwrap(); // Other setup wins the observation/install race.
    assert!(ctl.install_new(&spec).is_err());
    assert!(!runner.loaded.get());
}
