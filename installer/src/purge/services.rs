//! Exact selected service output admitted while ownership descriptors still exist.
use crate::Result;
use hive_setup_support::{
    files,
    removal::Action,
    runner::Runner,
    service::{Controller, Layout, Spec},
};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cleanup {
    layout: Layout,
    scopes: Vec<Scope>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scope {
    spec: Spec,
    actions: Vec<Action>,
}
impl Cleanup {
    pub(super) fn capture(layout: Layout, specs: Vec<Spec>) -> Result<Self> {
        let scopes = specs
            .into_iter()
            .map(|spec| {
                let actions = paths(&spec)
                    .iter()
                    .map(|p| Action::capture(p, "service"))
                    .collect::<std::io::Result<_>>()?;
                let scope = Scope { spec, actions };
                regular_files(&scope)?;
                Ok(scope)
            })
            .collect::<Result<_>>()?;
        Ok(Self { layout, scopes })
    }
    pub(super) fn validate(&self, prefix: &Path, components: &[String]) -> Result<()> {
        if self.scopes.len() > 256 {
            return Err("Too many service cleanup scopes".into());
        }
        for scope in &self.scopes {
            regular_files(scope)?;
            let spec = &scope.spec;
            spec.verify_layout(&self.layout, &spec.scope)?;
            if !components.contains(&spec.scope.product)
                || !["hivectl", "hive-daemon", "comb"]
                    .iter()
                    .any(|name| spec.program == prefix.join("bin").join(name))
                || scope
                    .actions
                    .iter()
                    .map(|a| &a.object.path)
                    .ne(paths(spec).iter())
            {
                return Err("Service cleanup is outside the selected installation".into());
            }
            for action in &scope.actions {
                action.validate()?;
            }
        }
        Ok(())
    }
    pub(super) fn preview(&self) {
        for scope in &self.scopes {
            for action in &scope.actions {
                eprintln!("Purge service output: {}", action.object.path.display());
            }
            eprintln!(
                "Retain service scope lock: {}",
                scope.spec.descriptor.with_extension("lock").display()
            );
        }
    }
    pub(super) fn len(&self) -> usize {
        self.scopes.len()
    }
    pub(super) fn remove(&mut self, index: usize, runner: &dyn Runner) -> Result<()> {
        let scope = &mut self.scopes[index];
        let _held = Controller {
            layout: &self.layout,
            scope: &scope.spec.scope,
            runner,
        }
        .hold_removed(&scope.spec)?;
        if files::read(&scope.spec.descriptor)?.is_some()
            || files::read(&scope.spec.unit)?.is_some()
        {
            return Err("Service was registered again; retaining its output".into());
        }
        for action in &mut scope.actions {
            action.remove()?;
        }
        Ok(())
    }
}
fn paths(spec: &Spec) -> [std::path::PathBuf; 3] {
    [
        spec.log.clone(),
        spec.descriptor.with_extension("failure.json"),
        spec.descriptor.with_extension("refresh.json"),
    ]
}
fn regular_files(scope: &Scope) -> Result<()> {
    if scope.actions.iter().any(|a| {
        a.object
            .identity
            .as_ref()
            .is_some_and(|i| !i.is_regular_file())
    }) {
        return Err(
            "Service output purge requires regular files, not directories or other objects".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use hive_setup_support::{
        runner::Output,
        service::{Platform, Scope as ServiceScope},
    };
    struct Stopped;
    impl Runner for Stopped {
        fn run(&self, _: &str, args: &[String]) -> std::io::Result<Output> {
            Ok(Output {
                code: i32::from(args[0] == "print" && args[1].contains("/io.hive.")) * 113,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }
    #[test]
    fn selected_service_output_is_removed_but_other_scopes_and_new_logs_survive() {
        for recreate in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().canonicalize().unwrap();
            let prefix = home.join("prefix");
            std::fs::create_dir_all(prefix.join("bin")).unwrap();
            std::fs::copy("/usr/bin/true", prefix.join("bin/hivectl")).unwrap();
            let layout = Layout {
                home: home.clone(),
                config: home.join("config"),
                state: home.join("state"),
                platform: Platform::Launchd,
            };
            let mut specs = vec![];
            for name in ["selected", "other"] {
                let scope = ServiceScope::new(
                    "hive",
                    &[&home.join(format!("{name}.yaml")), &home.join(name)],
                )
                .unwrap();
                let spec = Spec::new(
                    &layout,
                    scope,
                    &prefix.join("bin/hivectl"),
                    vec![],
                    None,
                    vec![],
                )
                .unwrap();
                for path in paths(&spec) {
                    files::directory(path.parent().unwrap()).unwrap();
                    files::write(&path, b"private service output", false).unwrap();
                }
                specs.push(spec);
            }
            let mut cleanup = Cleanup::capture(layout, vec![specs[0].clone()]).unwrap();
            cleanup.validate(&prefix, &["hive".into()]).unwrap();
            if recreate {
                std::fs::rename(&specs[0].log, home.join("old-log")).unwrap();
                files::write(&specs[0].log, b"new log", false).unwrap();
                assert!(cleanup.remove(0, &Stopped).is_err());
                assert_eq!(std::fs::read(&specs[0].log).unwrap(), b"new log");
            } else {
                cleanup.remove(0, &Stopped).unwrap();
                for path in paths(&specs[0]) {
                    assert!(!path.exists());
                }
                cleanup.remove(0, &Stopped).unwrap();
                assert!(specs[0].descriptor.with_extension("lock").exists());
                std::fs::create_dir(&specs[0].log).unwrap();
                let sentinel = specs[0].log.join("personal");
                std::fs::write(&sentinel, b"keep").unwrap();
                assert!(Cleanup::capture(cleanup.layout.clone(), vec![specs[0].clone()]).is_err());
                assert_eq!(std::fs::read(sentinel).unwrap(), b"keep");
            }
            for path in paths(&specs[1]) {
                assert_eq!(std::fs::read(path).unwrap(), b"private service output");
            }
        }
    }
}
