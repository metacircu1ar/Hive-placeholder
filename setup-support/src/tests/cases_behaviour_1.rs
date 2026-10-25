//! Behaviour regressions.
use super::*;

#[test]
fn foreign_jobs_and_edited_units_are_never_controlled() {
    let (_temp, layout, scope, spec) = fixture();
    let mock = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    mock.loaded.set(true);
    assert!(ctl.install(&spec).is_err());
    assert!(!spec.descriptor.exists());
    mock.loaded.set(false);
    ctl.install(&spec).unwrap();
    files::write(&spec.unit, b"foreign", true).unwrap();
    mock.calls.borrow_mut().clear();
    assert!(ctl.stop(&|| Ok(false)).is_err());
    assert!(ctl.uninstall(&|| Ok(false)).is_err());
    assert!(mock.calls.borrow().is_empty());
    assert_eq!(fs::read(&spec.unit).unwrap(), b"foreign");
}

#[test]
fn renderers_escape_literals_and_keep_stable_program_symlink() {
    let (_temp, layout, scope, spec) = fixture();
    let xml = String::from_utf8(spec.render().unwrap()).unwrap();
    assert!(xml.contains("&amp; &lt;tag&gt;"));
    assert!(xml.contains("SuccessfulExit</key><false/>"));
    let mut systemd = spec.clone();
    systemd.platform = Platform::Systemd;
    let unit = String::from_utf8(systemd.render().unwrap()).unwrap();
    assert!(unit.contains("$$variable %%specifier"));
    assert!(unit.contains("Restart=on-failure"));
    assert!(!unit.contains("--replace-embeddings"));
    systemd.arguments.push("line\nbreak".into());
    assert!(systemd.render().is_err());
    let stable = layout.home.join("comb");
    std::os::unix::fs::symlink("/bin/echo", &stable).unwrap();
    let spec = Spec::new(&layout, scope, &stable, vec!["serve".into()], None, vec![]).unwrap();
    assert_eq!(spec.program, stable);
}

#[test]
fn bounded_runner_handles_flood_hang_and_descendants_without_hanging() {
    let run = |body: &str, timeout| {
        runner::capture(
            std::process::Command::spawn,
            "/bin/sh",
            &["-c".into(), body.into()],
            timeout,
        )
    };
    // Success verifies capture/descendant cleanup, not sub-100ms OS scheduling.
    let success = Duration::from_secs(1);
    let timeout = Duration::from_millis(100);
    assert_eq!(run("printf ready", success).unwrap().stdout, "ready");
    let start = Instant::now();
    assert!(run("sleep 30", timeout).is_err());
    assert!(start.elapsed() < Duration::from_secs(2));
    let start = Instant::now();
    assert_eq!(run("sleep 30 & exit 0", success).unwrap().code, 0);
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(run("while :; do printf abcdefghijklmnop; done", timeout).is_err());
}

#[test]
fn private_records_reject_symlinks_and_hardlinks() {
    let (_temp, layout, _, _) = fixture();
    let path = layout.home.join("record");
    files::write(&path, b"ok", false).unwrap();
    let link = layout.home.join("link");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(files::read(&link).is_err());
    fs::remove_file(link).unwrap();
    fs::hard_link(&path, layout.home.join("hard")).unwrap();
    assert!(files::read(&path).is_err());
}

#[test]
fn startup_failures_distinguish_transient_and_suppressed_before_readiness() {
    let (_temp, _layout, _scope, spec) = fixture();
    assert!(!crate::managed::startup_failed(&spec, "recovery_required", false).unwrap());
    let transient = crate::managed::failure(&spec).unwrap().unwrap();
    assert!(!transient.suppressed);
    assert_eq!(transient.code, "recovery_required");
    assert!(crate::managed::startup_failed(&spec, "config_invalid", true).unwrap());
    assert!(crate::managed::failure(&spec).unwrap().unwrap().suppressed);
    assert_eq!(
        crate::managed::failure(&spec).unwrap().unwrap().code,
        "config_invalid"
    );
    crate::managed::clear_failure(&spec).unwrap();
    crate::managed::ready();
    assert!(!crate::managed::startup_failed(&spec, "config_invalid", true).unwrap());
    assert!(crate::managed::failure(&spec).unwrap().is_none());
}

#[test]
fn loaded_but_unready_is_restored_and_restart_errors_remain_typed() {
    let (_temp, layout, scope, spec) = fixture();
    let mock = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    ctl.install(&spec).unwrap();
    mock.loaded.set(true);
    ctl.paused(&|| Ok(mock.ready.get()), || Ok(())).unwrap();
    assert!(mock.ready.get());
    for fail_operation in [false, true] {
        mock.fail_start.set(false);
        ctl.start(&|| Ok(mock.ready.get())).unwrap();
        mock.fail_start.set(true);
        let error = ctl
            .paused(&|| Ok(mock.ready.get()), || {
                if fail_operation {
                    Err(crate::invalid("operation failure"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        let failure = error
            .get_ref()
            .unwrap()
            .downcast_ref::<service::RestartFailure>()
            .unwrap();
        assert_eq!(failure.operation.is_some(), fail_operation);
        assert!(failure.restart.to_string().contains("mock manager failure"));
    }
}

#[test]
fn scope_and_layout_resolve_user_symlinks_but_managed_records_do_not_follow_them() {
    let (_temp, mut layout, _, _) = fixture();
    let real = layout.home.join("real");
    fs::create_dir(&real).unwrap();
    let link = layout.home.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    layout.config = files::location(&link).unwrap();
    assert_eq!(
        files::absolute(&link.join("missing/deeper")).unwrap(),
        real.join("missing/deeper")
    );
    let config = real.join("config.yaml");
    fs::write(&config, "auth_profiles: {}").unwrap();
    let linked_config = layout.home.join("config-link.yaml");
    std::os::unix::fs::symlink(&config, &linked_config).unwrap();
    let scope = Scope::new("hive", &[&linked_config, &link.join("state")]).unwrap();
    assert_eq!(scope.paths[0], config);
    assert_eq!(
        scope,
        Scope::new("hive", &[&config, &real.join("state")]).unwrap()
    );
    let spec = Spec::new(
        &layout,
        scope.clone(),
        std::path::Path::new("/bin/echo"),
        vec![],
        None,
        vec![],
    )
    .unwrap();
    let mock = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &mock,
    };
    ctl.install(&spec).unwrap();
    assert!(ctl.status(&|| Ok(false)).unwrap().managed);
    spec.verify(&layout, &scope).unwrap();
    assert!(spec.descriptor.starts_with(real));
    assert!(files::absolute(&linked_config).is_err());
}

#[test]
fn identical_record_retry_repeats_a_failed_publication_barrier() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("record");
    files::FAIL_SYNC.with(|fault| *fault.borrow_mut() = Some(path.clone()));
    assert!(files::write(&path, b"published", false).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"published");
    // An identical retry cannot claim success while the barrier still fails.
    assert!(files::write(&path, b"published", false).is_err());
    files::FAIL_SYNC.with(|fault| *fault.borrow_mut() = None);
    files::SYNCED.with(|paths| paths.borrow_mut().clear());
    files::write(&path, b"published", false).unwrap();
    assert!(files::SYNCED.with(|paths| paths.borrow().contains(&path)));
}

#[test]
fn progress_is_visible_before_exit_and_owns_no_global_download_deadline() {
    use std::{
        path::PathBuf,
        process::{Child, Command, Stdio},
    };
    thread_local! { static PROGRESS_FILE: RefCell<Option<PathBuf>> = const { RefCell::new(None) }; }
    fn spawn(cmd: &mut Command) -> std::io::Result<Child> {
        let path = PROGRESS_FILE.with(|p| p.borrow().clone().unwrap());
        cmd.stderr(Stdio::from(fs::File::create(path)?));
        cmd.spawn()
    }
    let temp = tempfile::tempdir().unwrap();
    let progress = temp.path().join("progress");
    let ack = temp.path().join("ack");
    PROGRESS_FILE.with(|p| *p.borrow_mut() = Some(progress.clone()));
    let start = Instant::now();
    let seen = Cell::new(false);
    let output = runner::progress(spawn, "/bin/sh", &["-c".into(), "echo downloading >&2; sleep 0.1; while [ ! -f \"$1\" ]; do sleep 0.01; done; echo '{}'".into(), "fixture".into(), ack.to_string_lossy().into_owned()], &|| {
        if fs::read_to_string(&progress).is_ok_and(|s| s.contains("downloading")) {
            seen.set(true);
            fs::write(&ack, "seen before exit").unwrap();
        }
        start.elapsed() > Duration::from_secs(3)
    }).unwrap();
    assert!(seen.get());
    assert!(start.elapsed() >= Duration::from_millis(100));
    assert_eq!(output.stdout.trim(), "{}");
    assert_eq!(output.code, 0);
    assert!(output.stderr.is_empty());
    let cancel = runner::progress(
        Command::spawn,
        "/bin/sh",
        &["-c".into(), "sleep 30".into()],
        &|| true,
    )
    .unwrap_err();
    assert_eq!(cancel.kind(), std::io::ErrorKind::Interrupted);
}

#[test]
fn renderer_changes_do_not_confuse_owned_units_with_user_edits() {
    let (_temp, layout, scope, spec) = fixture();
    let runner = Mock::new();
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &runner,
    };
    ctl.install(&spec).unwrap();
    let mut old = spec.clone();
    old.unit_contents.push_str("\n<!-- previous renderer -->\n");
    files::write(&old.unit, old.unit_contents.as_bytes(), true).unwrap();
    files::write(&old.descriptor, &files::encode(&old).unwrap(), true).unwrap();
    assert!(ctl.status(&|| Ok(false)).unwrap().managed);
    ctl.start(&|| Ok(runner.ready.get())).unwrap();
    ctl.stop(&|| Ok(runner.ready.get())).unwrap();
    ctl.install(&spec).unwrap();
    assert_eq!(
        files::read(&spec.unit).unwrap().unwrap(),
        spec.unit_contents.as_bytes()
    );
    files::write(&old.unit, old.unit_contents.as_bytes(), true).unwrap();
    files::write(&old.descriptor, &files::encode(&old).unwrap(), true).unwrap();
    ctl.uninstall(&|| Ok(false)).unwrap();
    ctl.install(&spec).unwrap();
    files::write(&spec.unit, b"user edit", true).unwrap();
    assert!(ctl.install(&spec).is_err());
    assert!(ctl.uninstall(&|| Ok(false)).is_err());
}
