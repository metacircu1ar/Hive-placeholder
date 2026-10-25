#![allow(clippy::unwrap_used)] // Test fixtures and assertions fail on unexpected errors.
mod systemd;
use crate::{
    environment, files,
    runner::{self, Output, Runner},
    service::{self, Controller, Layout, Platform, Scope, Spec},
};
use std::{
    cell::{Cell, RefCell},
    fs,
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};

struct Mock {
    loaded: Cell<bool>,
    ready: Cell<bool>,
    failure: Cell<bool>,
    unavailable: Cell<bool>,
    fail_start: Cell<bool>,
    calls: RefCell<Vec<Vec<String>>>,
}
impl Mock {
    fn new() -> Self {
        Self {
            loaded: Cell::new(false),
            ready: Cell::new(false),
            failure: Cell::new(false),
            unavailable: Cell::new(false),
            fail_start: Cell::new(false),
            calls: RefCell::new(vec![]),
        }
    }
}
impl Runner for Mock {
    fn run(&self, _: &str, args: &[String]) -> crate::Result<Output> {
        self.calls.borrow_mut().push(args.to_vec());
        let mut code = 0;
        match args[0].as_str() {
            "print-disabled" => {
                if self.unavailable.get() {
                    code = 112;
                }
            }
            "print" => {
                if !self.loaded.get() {
                    code = 113;
                }
            }
            "bootstrap" | "kickstart" => {
                if self.fail_start.get() {
                    code = 5;
                } else {
                    self.loaded.set(true);
                    self.ready.set(true);
                }
            }
            "bootout" => {
                if self.failure.get() {
                    code = 1;
                } else {
                    self.loaded.set(false);
                    self.ready.set(false);
                }
            }
            "enable" => (),
            unknown => panic!("Unexpected manager call: {unknown}"),
        }
        Ok(Output {
            stderr: "mock manager failure".into(),
            code,
            stdout: if args[0] == "print-disabled" {
                "\n\tdisabled services = {\n\t}\n".into()
            } else {
                String::new()
            },
        })
    }
}
fn fixture() -> (tempfile::TempDir, Layout, Scope, Spec) {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let layout = Layout {
        config: home.join("config"),
        state: home.join("state"),
        home: home.clone(),
        platform: Platform::Launchd,
    };
    let scope = Scope::new("comb", &[&home.join("Comb")]).unwrap();
    let spec = Spec::new(
        &layout,
        scope.clone(),
        std::path::Path::new("/bin/echo"),
        vec![
            "serve".into(),
            "a 'quoted' $variable %specifier & <tag>".into(),
        ],
        None,
        vec![],
    )
    .unwrap();
    (temp, layout, scope, spec)
}

mod cases_behaviour_1;
mod cases_indexing_1;
mod cases_lifecycle_1;
mod cases_requests_1;
mod cases_services_1;
