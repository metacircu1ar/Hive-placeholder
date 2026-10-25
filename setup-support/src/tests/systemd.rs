use super::*;
struct Systemd {
    unit: std::path::PathBuf,
    running: Cell<bool>,
    enabled: Cell<bool>,
    fail_reload: Cell<bool>,
    calls: RefCell<Vec<Vec<String>>>,
}
impl Runner for Systemd {
    fn run(&self, program: &str, args: &[String]) -> crate::Result<Output> {
        self.calls.borrow_mut().push(args.to_vec());
        if program.ends_with("loginctl") {
            return Ok(Output {
                stderr: "mock manager failure".into(),
                code: 0,
                stdout: "Linger=no\n".into(),
            });
        }
        assert_eq!(args[0], "--user");
        let mut code = 0;
        let mut stdout = String::new();
        match args[1].as_str() {
            "show" if args[2] == "--property=Version" => (),
            "show" => {
                if self.unit.exists() {
                    stdout = format!(
                        "LoadState=loaded\nActiveState={}\nFragmentPath={}\n",
                        if self.running.get() {
                            "active"
                        } else {
                            "inactive"
                        },
                        self.unit.display()
                    );
                } else {
                    stdout = "LoadState=not-found\n".into();
                }
            }
            "enable" => {
                assert_eq!(args[2], self.unit.to_str().unwrap());
                self.enabled.set(true);
            }
            "disable" => self.enabled.set(false),
            "start" => self.running.set(true),
            "stop" => self.running.set(false),
            "is-enabled" => {
                stdout = if self.enabled.get() {
                    "enabled"
                } else {
                    code = 1;
                    "disabled"
                }
                .into();
            }
            "daemon-reload" => {
                if self.fail_reload.get() {
                    code = 1;
                }
            }
            "reset-failed" => (),
            unknown => panic!("Unexpected systemctl operation {unknown}"),
        }
        Ok(Output {
            stderr: "mock manager failure".into(),
            code,
            stdout,
        })
    }
}
#[test]
fn systemd_links_exact_absolute_unit_resets_failures_and_reports_login_lifetime() {
    let (_temp, mut layout, scope, _) = fixture();
    layout.platform = Platform::Systemd;
    let spec = Spec::new(
        &layout,
        scope.clone(),
        std::path::Path::new("/bin/echo"),
        vec!["serve".into()],
        None,
        vec![],
    )
    .unwrap();
    let runner = Systemd {
        unit: spec.unit.clone(),
        running: Cell::new(false),
        enabled: Cell::new(false),
        fail_reload: Cell::new(false),
        calls: RefCell::new(vec![]),
    };
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &runner,
    };
    let health = || Ok(runner.running.get());
    ctl.install(&spec).unwrap();
    ctl.start(&health).unwrap();
    let status = ctl.status(&health).unwrap();
    assert_eq!(status.enabled, Some(true));
    assert!(status.ready);
    assert!(status.note.unwrap().contains("Stops at logout"));
    assert!(
        runner
            .calls
            .borrow()
            .iter()
            .any(|a| a.get(1).is_some_and(|s| s == "reset-failed"))
    );
    ctl.uninstall(&health).unwrap();
    assert!(!runner.enabled.get());
    assert!(!runner.running.get());
    assert!(!spec.unit.exists());
}

#[test]
fn systemd_logs_are_unquoted_and_native_parser_accepts_unit() {
    let (_temp, mut layout, scope, _) = fixture();
    layout.platform = Platform::Systemd;
    let mut spec = Spec::new(
        &layout,
        scope,
        std::path::Path::new("/bin/echo"),
        vec![],
        None,
        vec![],
    )
    .unwrap();
    spec.log = layout.home.join("log with spaces % value.log");
    let unit = String::from_utf8(spec.render().unwrap()).unwrap();
    assert!(unit.contains(&format!(
        "StandardOutput=append:{}\n",
        spec.log.display().to_string().replace('%', "%%")
    )));
    assert!(unit.contains(&format!(
        "StandardError=append:{}\n",
        spec.log.display().to_string().replace('%', "%%")
    )));
    #[cfg(target_os = "linux")]
    {
        let path = layout.home.join("test.service");
        fs::write(&path, unit).unwrap();
        // Parser verification needs no running user bus; --man=no skips man-page lookup.
        let output = std::process::Command::new("systemd-analyze")
            .args(["--user", "verify", "--man=no"])
            .arg(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("Failed to parse"));
    }
}

#[test]
fn setup_starts_registered_stopped_units_but_preserves_starting_units() {
    let (_temp, mut layout, scope, _) = fixture();
    layout.platform = Platform::Systemd;
    let spec = Spec::new(
        &layout,
        scope.clone(),
        std::path::Path::new("/bin/echo"),
        vec![],
        None,
        vec![],
    )
    .unwrap();
    let runner = Systemd {
        unit: spec.unit.clone(),
        running: Cell::new(false),
        enabled: Cell::new(false),
        fail_reload: Cell::new(false),
        calls: RefCell::new(vec![]),
    };
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &runner,
    };
    let health = || Ok(runner.running.get());
    ctl.install(&spec).unwrap();
    assert!(ctl.status(&health).unwrap().registered);
    assert!(ctl.ensure_started(&health).unwrap());
    ctl.stop(&health).unwrap();
    assert!(ctl.ensure_started(&health).unwrap());
    // A loaded job without a healthy socket may still be starting. Never kick it.
    runner.calls.borrow_mut().clear();
    assert!(!ctl.ensure_started(&|| Ok(false)).unwrap());
    assert!(
        !runner
            .calls
            .borrow()
            .iter()
            .any(|a| a.get(1).is_some_and(|s| s == "start"))
    );
}

#[test]
fn matching_artifacts_still_retry_directory_barriers_and_systemd_reload() {
    let (_temp, mut layout, scope, _) = fixture();
    layout.platform = Platform::Systemd;
    let spec = Spec::new(
        &layout,
        scope.clone(),
        std::path::Path::new("/bin/echo"),
        vec![],
        None,
        vec![],
    )
    .unwrap();
    let runner = Systemd {
        unit: spec.unit.clone(),
        running: Cell::new(false),
        enabled: Cell::new(false),
        fail_reload: Cell::new(false),
        calls: RefCell::new(vec![]),
    };
    let ctl = Controller {
        layout: &layout,
        scope: &scope,
        runner: &runner,
    };
    ctl.install(&spec).unwrap();
    let mut old = spec.clone();
    old.unit_contents.push_str("\n# Former renderer\n");
    files::write(&old.unit, old.unit_contents.as_bytes(), true).unwrap();
    files::write(&old.descriptor, &files::encode(&old).unwrap(), true).unwrap();
    runner.fail_reload.set(true);
    assert!(ctl.install(&spec).is_err());
    // Both new artifacts were written, but the old manager cache is still live.
    spec.verify(&layout, &scope).unwrap();
    let pending = spec.descriptor.with_extension("refresh.json");
    assert!(pending.exists());
    runner.fail_reload.set(false);
    crate::files::FAIL_SYNC.with(|fault| *fault.borrow_mut() = Some(spec.unit.clone()));
    assert!(ctl.install(&spec).is_err());
    crate::files::FAIL_SYNC.with(|fault| *fault.borrow_mut() = None);
    assert!(pending.exists());
    runner.calls.borrow_mut().clear();
    ctl.start(&|| Ok(runner.running.get())).unwrap();
    let calls = runner.calls.borrow();
    let reload = calls
        .iter()
        .position(|a| a.get(1).is_some_and(|s| s == "daemon-reload"))
        .unwrap();
    let start = calls
        .iter()
        .position(|a| a.get(1).is_some_and(|s| s == "start"))
        .unwrap();
    assert!(reload < start);
    assert!(!pending.exists());
}
