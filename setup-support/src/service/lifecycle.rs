use super::{
    Layout, Scope, Spec, load,
    manager::{Manager, State},
};
use crate::{Result, files, invalid, runner::Runner};
use serde::Serialize;
use std::{
    fs::OpenOptions,
    os::unix::fs::OpenOptionsExt,
    time::{Duration, Instant},
};
pub type Health<'a> = dyn Fn() -> Result<bool> + 'a;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartDecision {
    Started,
    AlreadyLoaded,
    PreservedStopped,
}
#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)] // Independent observations, not a lifecycle state machine.
pub struct Status {
    pub managed: bool,
    pub outdated_unit: bool,
    pub manager_available: bool,
    pub registered: bool,
    pub enabled: Option<bool>,
    pub ready: bool,
    pub label: String,
    pub missing_environment: Vec<String>,
    pub failure: Option<crate::managed::Failure>,
    pub log: std::path::PathBuf,
    pub note: Option<String>,
    pub manual_start: String,
    pub service_path: Option<String>,
    pub warnings: Vec<String>,
}
pub struct Controller<'a> {
    pub layout: &'a Layout,
    pub scope: &'a Scope,
    pub runner: &'a dyn Runner,
}
impl Controller<'_> {
    pub(super) fn manager(&self) -> Manager<'_> {
        Manager {
            layout: self.layout,
            runner: self.runner,
        }
    }
    pub(super) fn lock(&self) -> Result<crate::locks::Guard> {
        let file = self.lock_without_recovery()?;
        self.recover_refresh()?;
        Ok(file)
    }
    pub(super) fn lock_without_recovery(&self) -> Result<crate::locks::Guard> {
        let path = self.layout.descriptor(self.scope)?.with_extension("lock");
        files::directory(
            path.parent()
                .ok_or_else(|| invalid("Missing lock parent."))?,
        )?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)?;
        files::read(&path)?;
        crate::locks::Guard::exclusive(file)
            .map_err(|error| crate::context("Cannot acquire setup/service scope lock", error))
    }
    pub fn status(&self, health: &Health<'_>) -> Result<Status> {
        let spec = load(self.layout, self.scope)?;
        let manager = self.manager();
        let available = manager.available()?;
        Ok(Status {
            manual_start: self.scope.foreground_command()?,
            service_path: spec
                .as_ref()
                .and_then(|s| s.environment.get("PATH").cloned()),
            warnings: vec![],
            managed: spec.is_some(),
            outdated_unit: spec
                .as_ref()
                .map(|s| Ok::<_, std::io::Error>(s.unit_contents.as_bytes() != s.render()?))
                .transpose()?
                .unwrap_or(false),
            enabled: if available && spec.is_some() {
                manager.enabled(self.scope)?
            } else {
                None
            },
            manager_available: available,
            registered: if available && spec.is_some() {
                manager.state(self.scope)? != State::Absent
            } else {
                false
            },
            ready: health()?,
            label: self.scope.label()?,
            missing_environment: spec
                .as_ref()
                .map(Spec::missing_environment)
                .transpose()?
                .unwrap_or_default(),
            failure: spec
                .as_ref()
                .map(crate::managed::failure)
                .transpose()?
                .flatten(),
            log: self.layout.log(self.scope)?,
            note: if available {
                manager.login_note()?
            } else {
                Some(
                    "Not managed: no user service manager. Run the daemon in the foreground."
                        .into(),
                )
            },
        })
    }
    pub fn install(&self, spec: &Spec) -> Result<()> {
        let _prefix = super::registration::guard(&spec.program)?;
        let _lock = self.lock()?;
        self.install_locked(&spec.rendered()?)
    }
    pub fn install_new(&self, spec: &Spec) -> Result<()> {
        let _prefix = super::registration::guard(&spec.program)?;
        let _lock = self.lock()?;
        if files::read(&spec.descriptor)?.is_some() {
            return Err(invalid(
                "Service appeared during setup; retry to preserve its existing running state.",
            ));
        }
        self.install_locked(&spec.rendered()?)
    }
    pub(super) fn install_locked(&self, spec: &Spec) -> Result<()> {
        if spec.scope != *self.scope {
            return Err(invalid("Service scope mismatch."));
        }
        spec.check_environment()?;
        let manager = self.manager();
        if !manager.available()? {
            return Err(invalid(
                "Not managed: no user service manager; use the displayed foreground command.",
            ));
        }
        let old: Option<Spec> = files::json(&spec.descriptor)?;
        if let Some(old) = old {
            if old != *spec {
                self.refresh_unit(&old, spec)?;
            }
            // An interrupted install may have published only its descriptor.
            if let Some(bytes) = files::read(&spec.unit)?
                && bytes != spec.unit_contents.as_bytes()
            {
                return Err(invalid("Service unit was edited; refusing replacement."));
            }
        } else if files::read(&spec.unit)?.is_some() || manager.state(self.scope)? != State::Absent
        {
            return Err(invalid("A foreign unit or job occupies the service name."));
        }
        files::log(&spec.log)?;
        files::write(&spec.descriptor, &files::encode(spec)?, false)?;
        files::write(&spec.unit, spec.unit_contents.as_bytes(), false)?;
        manager.install(spec)
    }
    pub fn start(&self, health: &Health<'_>) -> Result<()> {
        let _lock = self.lock()?;
        let spec = load(self.layout, self.scope)?
            .ok_or_else(|| invalid("Service is not installed; run service install."))?;
        self.start_owned(&spec, health)
    }
    /// Setup may start a stopped service, but must not kick an already loaded
    /// service which is still initializing. Inspect under the same scope lock.
    pub fn ensure_started(&self, health: &Health<'_>) -> Result<bool> {
        Ok(self.start_for_setup(health, false)? == StartDecision::Started)
    }
    pub fn start_for_setup(
        &self,
        health: &Health<'_>,
        preserve_stopped: bool,
    ) -> Result<StartDecision> {
        let _lock = self.lock()?;
        let spec = load(self.layout, self.scope)?
            .ok_or_else(|| invalid("Service is not installed; run service install."))?;
        if !self.manager().available()? {
            return Err(invalid(
                "Cannot determine setup service state: user manager unavailable.",
            ));
        }
        if self.manager().state(self.scope)? == State::Loaded {
            return Ok(StartDecision::AlreadyLoaded);
        }
        let desired = spec.rendered()?;
        if desired != spec {
            self.install_locked(&desired)?;
        }
        if preserve_stopped {
            spec.check_environment()?;
            if crate::managed::failure(&spec)?.is_some() {
                return Err(invalid(
                    "Stopped service has a recorded startup failure; correct it and run \
                        service start.",
                ));
            }
            if health()? {
                return Err(invalid(
                    "An unmanaged instance is serving the stopped service scope.",
                ));
            }
            return Ok(StartDecision::PreservedStopped);
        }
        self.start_owned(&desired, health)?;
        Ok(StartDecision::Started)
    }
    pub(super) fn start_owned(&self, spec: &Spec, health: &Health<'_>) -> Result<()> {
        spec.verify(self.layout, self.scope)?;
        spec.check_environment()?;
        if !self.manager().available()? {
            return Err(invalid("User service manager is unavailable."));
        }
        if health()? {
            if self.manager().state(self.scope)? != State::Loaded {
                return Err(invalid(
                    "An unmanaged instance is using this scope; stop it first.",
                ));
            }
            return Ok(());
        }
        crate::managed::clear_failure(spec)?;
        self.manager().start(spec)?;
        let deadline = Instant::now() + START_TIMEOUT;
        while Instant::now() < deadline {
            if health()? {
                return Ok(());
            }
            if let Some(failure) = crate::managed::failure(spec)?
                && failure.suppressed
            {
                return Err(invalid(format!(
                    "Service startup stopped: {}. Inspect {} and correct its configuration \
                        before service start.",
                    failure.code,
                    spec.log.display()
                )));
            }
            std::thread::sleep(START_POLL);
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!(
                "Service has not become ready; inspect {}. Startup may still be running.",
                spec.log.display()
            ),
        ))
    }
    pub fn stop(&self, health: &Health<'_>) -> Result<()> {
        let _lock = self.lock()?;
        let spec =
            load(self.layout, self.scope)?.ok_or_else(|| invalid("Service is not installed."))?;
        self.stop_owned(&spec, health)
    }
    pub(super) fn stop_owned(&self, spec: &Spec, health: &Health<'_>) -> Result<()> {
        spec.verify(self.layout, self.scope)?;
        if !self.manager().available()? {
            return Err(invalid(
                "Cannot confirm stop: user service manager unavailable.",
            ));
        }
        self.manager().stop(spec)?;
        let deadline = Instant::now() + STOP_TIMEOUT;
        while Instant::now() < deadline {
            if self.manager().state(self.scope)? != State::Loaded {
                if health()? {
                    return Err(invalid(
                        "Manager stopped but the scope is still serving; refusing mutation.",
                    ));
                }
                return Ok(());
            }
            std::thread::sleep(STOP_POLL);
        }
        Err(invalid(
            "Service stop is unconfirmed; no files were changed.",
        ))
    }
    pub fn uninstall(&self, health: &Health<'_>) -> Result<()> {
        let _lock = self.lock()?;
        self.uninstall_locked(health)
    }
    pub(super) fn uninstall_locked(&self, health: &Health<'_>) -> Result<()> {
        let Some(spec): Option<Spec> = files::json(&self.layout.descriptor(self.scope)?)? else {
            return Ok(());
        };
        spec.verify_record(self.layout, self.scope)?;
        if files::read(&spec.unit)?.is_none() {
            // Resume an uninstall interrupted between unit and descriptor removal.
            if !self.manager().available()?
                || self.manager().state(self.scope)? == State::Loaded
                || health()?
            {
                return Err(invalid(
                    "Unit is missing but service stop cannot be confirmed; refusing ownership \
                        cleanup.",
                ));
            }
            files::remove(&spec.descriptor, &files::encode(&spec)?)?;
            return self.manager().reload();
        }
        self.stop_owned(&spec, health)?;
        self.manager().uninstall(&spec)?;
        files::remove(&spec.unit, spec.unit_contents.as_bytes())?;
        files::remove(&spec.descriptor, &files::encode(&spec)?)?;
        self.manager().reload()
    }
    /// Hold the service lock across a root mutation. Restart only a service which
    #[doc = " was loaded (including still starting) beforehand; callers must release \
        root/store locks in `apply`."]
    pub fn paused<T>(&self, health: &Health<'_>, apply: impl FnOnce() -> Result<T>) -> Result<T> {
        let _lock = self.lock()?;
        let spec = load(self.layout, self.scope)?;
        let running = health()?;
        if running && spec.is_none() {
            return Err(invalid(
                "Unmanaged instance is serving; stop it before changing its root.",
            ));
        }
        let mut restore = false;
        if let Some(spec) = &spec {
            restore = self.manager().state(self.scope)? == State::Loaded;
            if running && !restore {
                return Err(invalid(
                    "Cannot attribute the live instance to the owned service.",
                ));
            }
            self.stop_owned(spec, health)?;
        }
        let result = apply();
        if restore
            && let Some(spec) = &spec
            && let Err(restart) = self
                .install_locked(&spec.rendered()?)
                .and_then(|()| self.start_owned(&spec.rendered()?, health))
        {
            return Err(std::io::Error::other(RestartFailure {
                operation: result.err(),
                restart,
            }));
        }
        result
    }
}

/// Retain the original operation's typed error through the shared service layer.
#[derive(Debug)]
pub struct RestartFailure {
    pub operation: Option<std::io::Error>,
    pub restart: std::io::Error,
}
impl std::fmt::Display for RestartFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(error) = &self.operation {
            write!(
                f,
                "Root operation failed: {error}; service restart failed: {}",
                self.restart
            )
        } else {
            write!(
                f,
                "Root operation committed; service restart failed: {}",
                self.restart
            )
        }
    }
}
impl std::error::Error for RestartFailure {}

const START_TIMEOUT: std::time::Duration = Duration::from_secs(60);

const START_POLL: std::time::Duration = Duration::from_millis(100);

const STOP_TIMEOUT: std::time::Duration = Duration::from_secs(35);

const STOP_POLL: std::time::Duration = Duration::from_millis(100);
