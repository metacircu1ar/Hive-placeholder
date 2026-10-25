use super::*;
use crate::cli::setup::publication::tests as faults;
use atomic_fs::Step;
use hive_setup_support::{
    files,
    runner::{Output, Runner},
    service::{Layout, Platform, Scope, Spec},
};
use std::{cell::Cell, error::Error as _, fs, path::Path};
const OLD: &str = "auth_profiles: {}\n# old\n";
const NEW: &str = "auth_profiles: {}\n# new\n";
struct Manager {
    unit: PathBuf,
    running: Cell<bool>,
    fail_start: bool,
    starts: Cell<usize>,
    stops: Cell<usize>,
}
impl Runner for Manager {
    fn run(&self, program: &str, args: &[String]) -> io::Result<Output> {
        assert_eq!(program, "/usr/bin/systemctl");
        assert_eq!(args[0], "--user");
        let mut stdout = String::new();
        match args[1].as_str() {
            "show" if args[2] == "--property=Version" => (),
            "show" => {
                stdout = format!(
                    "LoadState=loaded\nActiveState={}\nFragmentPath={}\n",
                    if self.running.get() {
                        "active"
                    } else {
                        "inactive"
                    },
                    self.unit.display()
                );
            }
            "enable" | "daemon-reload" | "reset-failed" => (),
            "start" => {
                self.starts.set(self.starts.get() + 1);
                if self.fail_start {
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
                self.running.set(true);
            }
            "stop" => {
                self.stops.set(self.stops.get() + 1);
                self.running.set(false);
            }
            other => panic!("Unexpected service manager operation {other}"),
        }
        Ok(Output {
            code: 0,
            stdout,
            stderr: String::new(),
        })
    }
}
struct Fixture {
    _temp: tempfile::TempDir,
    path: PathBuf,
    layout: Layout,
    scope: Scope,
    manager: Manager,
}
impl Fixture {
    fn new(fail_start: bool) -> std::result::Result<Self, Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let path = root.join("config.yaml");
        fs::write(&path, OLD)?;
        let layout = Layout {
            home: root.clone(),
            config: root.join("cfg"),
            state: root.join("state"),
            platform: Platform::Systemd,
        };
        let scope = Scope::new("hive", &[&path, &root])?;
        let spec = Spec::new(
            &layout,
            scope.clone(),
            Path::new("/bin/echo"),
            vec!["daemon".into(), "run".into()],
            None,
            vec![],
        )?;
        files::write(&spec.descriptor, &files::encode(&spec)?, false)?;
        files::write(&spec.unit, &spec.render()?, false)?;
        let manager = Manager {
            unit: spec.unit,
            running: Cell::new(true),
            fail_start,
            starts: Cell::new(0),
            stops: Cell::new(0),
        };
        Ok(Self {
            _temp: temp,
            path,
            layout,
            scope,
            manager,
        })
    }
    fn publish(&self, snapshot: &Snapshot) -> Result<Option<PathBuf>> {
        paused(
            &Controller {
                layout: &self.layout,
                scope: &self.scope,
                runner: &self.manager,
            },
            &|| Ok(self.manager.running.get()),
            snapshot,
            NEW,
        )
    }
}

#[test]
fn managed_restart_preserves_conflicts_and_accepted_native_publication_failures() {
    for fail_restart in [false, true] {
        for accepted in [false, true] {
            let f = Fixture::new(fail_restart).unwrap();
            let snapshot = Snapshot::inspect(&f.path).unwrap();
            let target = f.path.clone();
            let error = faults::with(
                move |at, step| {
                    if at == target {
                        if accepted && step == Step::BeforeDirectorySync {
                            return Err(io::Error::from_raw_os_error(libc::ENOSPC));
                        }
                        if !accepted && step == Step::BeforePublish {
                            fs::write(at, "auth_profiles: {}\n# editor\n")?;
                        }
                    }
                    Ok(())
                },
                || f.publish(&snapshot),
            )
            .unwrap_err();
            assert_eq!(error.code(), "config_invalid");
            let outer = error.source().unwrap().downcast_ref::<io::Error>().unwrap();
            let operation = if fail_restart {
                let failure = outer
                    .get_ref()
                    .unwrap()
                    .downcast_ref::<RestartFailure>()
                    .unwrap();
                assert_eq!(failure.restart.raw_os_error(), Some(libc::EIO));
                assert!(error.message().contains("Daemon restart also failed"));
                failure.operation.as_ref().unwrap()
            } else {
                outer
            };
            let public = operation
                .get_ref()
                .unwrap()
                .downcast_ref::<Error>()
                .unwrap();
            assert_eq!(public.code(), "config_invalid");
            let evidence = faults::evidence(public).unwrap();
            assert_eq!(evidence.published(), accepted);
            if accepted {
                assert_eq!(evidence.io_error().raw_os_error(), Some(libc::ENOSPC));
            }
            assert_eq!(
                fs::read_to_string(&f.path).unwrap(),
                if accepted {
                    NEW
                } else {
                    "auth_profiles: {}\n# editor\n"
                }
            );
            assert_eq!(f.manager.starts.get(), 1);
            assert_eq!(f.manager.stops.get(), 1);
            assert_eq!(f.manager.running.get(), !fail_restart);
        }
    }
}

#[test]
fn successful_publication_with_failed_restart_keeps_a_distinct_outcome() {
    let f = Fixture::new(true).unwrap();
    let snapshot = Snapshot::inspect(&f.path).unwrap();
    let error = f.publish(&snapshot).unwrap_err();
    assert_eq!(error.code(), "state_io");
    assert!(error.message().contains("Configuration is published"));
    let failure = error
        .source()
        .unwrap()
        .downcast_ref::<io::Error>()
        .unwrap()
        .get_ref()
        .unwrap()
        .downcast_ref::<RestartFailure>()
        .unwrap();
    assert!(failure.operation.is_none());
    assert_eq!(failure.restart.raw_os_error(), Some(libc::EIO));
    assert_eq!(fs::read_to_string(&f.path).unwrap(), NEW);
    assert!(!f.manager.running.get());
}

#[test]
fn managed_publication_retains_ambiguous_preparation_cleanup_evidence() {
    let f = Fixture::new(false).unwrap();
    let snapshot = Snapshot::inspect(&f.path).unwrap();
    let target = f.path.clone();
    let error = faults::with(
        move |at, step| {
            if at == target && step == Step::Written {
                for entry in fs::read_dir(at.parent().unwrap())? {
                    let entry = entry?;
                    if entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".hive-tmp-")
                    {
                        fs::remove_file(entry.path())?;
                        fs::write(entry.path(), "foreign preparation")?;
                    }
                }
            }
            Ok(())
        },
        || f.publish(&snapshot),
    )
    .unwrap_err();
    let operation = error
        .source()
        .unwrap()
        .downcast_ref::<io::Error>()
        .unwrap()
        .get_ref()
        .unwrap()
        .downcast_ref::<Error>()
        .unwrap();
    let evidence = faults::evidence(operation).unwrap();
    assert!(!evidence.published());
    assert!(evidence.cleanup_error().is_some());
    assert_eq!(
        fs::read_to_string(evidence.retained().as_ref().unwrap()).unwrap(),
        "foreign preparation"
    );
    assert_eq!(fs::read_to_string(&f.path).unwrap(), OLD);
    assert!(f.manager.running.get());
}
