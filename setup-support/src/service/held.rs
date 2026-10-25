//! Retained scope guards for installer transactions spanning several services.
use super::{Controller, Health, Layout, Spec, load, manager::State};
use crate::{Result, files, invalid, runner::Runner};

pub struct Held {
    layout: Layout,
    spec: Spec,
    _lock: crate::locks::Guard,
}
impl Controller<'_> {
    /// Purge exact retained output after uninstall; never revive a stale refresh journal.
    pub fn hold_removed(&self, expected: &Spec) -> Result<Held> {
        let lock = self.lock_without_recovery()?;
        expected.verify_layout(self.layout, self.scope)?;
        if files::read(&expected.descriptor)?.is_some()
            || files::read(&expected.unit)?.is_some()
            || !self.manager().available()?
            || self.manager().state(self.scope)? == State::Loaded
        {
            return Err(invalid(
                "Service scope is registered, running or unobservable; retain its output.",
            ));
        }
        Ok(Held {
            layout: self.layout.clone(),
            spec: expected.clone(),
            _lock: lock,
        })
    }
    pub fn hold(&self) -> Result<Held> {
        let lock = self.lock()?;
        let spec = load(self.layout, self.scope)?.ok_or_else(|| {
            invalid("Owned service disappeared before the installation lock was acquired.")
        })?;
        Ok(Held {
            layout: self.layout.clone(),
            spec,
            _lock: lock,
        })
    }
    /// Recover a descriptor retained in a private installer journal. Missing
    /// records are allowed only after the corresponding unit/job is gone.
    pub fn hold_expected(&self, expected: &Spec) -> Result<Held> {
        let lock = self.lock()?;
        expected.verify_layout(self.layout, self.scope)?;
        match files::json::<Spec>(&expected.descriptor)? {
            Some(current) if current == *expected => {
                expected.verify_record(self.layout, self.scope)?;
                if files::read(&expected.unit)?.is_some() {
                    expected.verify(self.layout, self.scope)?;
                }
            }
            Some(_) => return Err(invalid("Service changed since the pending installation.")),
            None => {
                if files::read(&expected.unit)?.is_some()
                    || !self.manager().available()?
                    || self.manager().state(self.scope)? == State::Loaded
                {
                    return Err(invalid(
                        "Cannot recover a removed service while its unit/job remains.",
                    ));
                }
            }
        }
        Ok(Held {
            layout: self.layout.clone(),
            spec: expected.clone(),
            _lock: lock,
        })
    }
    pub fn hold_transition(&self, old: &Spec, next: &Spec) -> Result<Held> {
        let lock = self.lock()?;
        check_transition(self.layout, self.scope, old, next)?;
        let spec = files::json::<Spec>(&old.descriptor)?
            .ok_or_else(|| invalid("Service record disappeared during upgrade."))?;
        Ok(Held {
            layout: self.layout.clone(),
            spec,
            _lock: lock,
        })
    }
}
impl Held {
    pub fn spec(&self) -> &Spec {
        &self.spec
    }
    fn controller<'a>(&'a self, runner: &'a dyn Runner) -> Controller<'a> {
        Controller {
            layout: &self.layout,
            scope: &self.spec.scope,
            runner,
        }
    }
    pub fn loaded(&self, runner: &dyn Runner) -> Result<bool> {
        self.spec.verify(&self.layout, &self.spec.scope)?;
        let controller = self.controller(runner);
        if !controller.manager().available()? {
            return Err(invalid(
                "Cannot establish the owned service state: user manager unavailable.",
            ));
        }
        Ok(controller.manager().state(&self.spec.scope)? == State::Loaded)
    }
    /// Publish a journaled artifact only while stopped, recovering either half
    /// of a unit/descriptor replacement. The caller retains the outer journal.
    pub fn restore_unit(
        &mut self,
        runner: &dyn Runner,
        old: &Spec,
        next: &Spec,
        published: bool,
    ) -> Result<()> {
        self.spec = restore_artifact(&self.layout, &self.spec.scope, runner, old, next, published)?;
        Ok(())
    }

    pub fn stop(&self, runner: &dyn Runner, health: &Health<'_>) -> Result<()> {
        self.controller(runner).stop_owned(&self.spec, health)
    }
    pub fn start(&self, runner: &dyn Runner, health: &Health<'_>) -> Result<()> {
        self.controller(runner).start_owned(&self.spec, health)
    }
    pub fn uninstall(&self, runner: &dyn Runner, health: &Health<'_>) -> Result<()> {
        self.controller(runner).uninstall_locked(health)
    }
    /// Restore precisely the descriptor retained by this guard, never inferred settings.
    pub fn install(&self, runner: &dyn Runner) -> Result<()> {
        self.controller(runner).install_locked(&self.spec)
    }
}

fn check_transition(layout: &Layout, scope: &super::Scope, old: &Spec, next: &Spec) -> Result<()> {
    old.verify_layout(layout, scope)?;
    next.verify_layout(layout, scope)?;
    let mut same = old.clone();
    same.unit_contents.clone_from(&next.unit_contents);
    if same != *next {
        return Err(invalid(
            "Unit refresh cannot change the service scope or invocation.",
        ));
    }
    let current = files::json::<Spec>(&old.descriptor)?
        .ok_or_else(|| invalid("Service record disappeared during upgrade."))?;
    let unit = files::read(&old.unit)?
        .ok_or_else(|| invalid("Service unit disappeared during upgrade."))?;
    if (current != *old && current != *next)
        || (unit != old.unit_contents.as_bytes() && unit != next.unit_contents.as_bytes())
    {
        return Err(invalid(
            "Service was edited during upgrade; refusing artifact recovery.",
        ));
    }
    Ok(())
}

pub(super) fn restore_artifact(
    layout: &Layout,
    scope: &super::Scope,
    runner: &dyn Runner,
    old: &Spec,
    next: &Spec,
    published: bool,
) -> Result<Spec> {
    check_transition(layout, scope, old, next)?;
    let desired = if published { next } else { old };
    let current = files::json::<Spec>(&old.descriptor)?
        .ok_or_else(|| invalid("Service record disappeared during upgrade."))?;
    if current == *desired
        && files::read(&desired.unit)?.as_deref() == Some(desired.unit_contents.as_bytes())
    {
        // Equal bytes do not prove the prior directory barrier or manager reload
        // completed. A durable journal still requires both before it can clear.
        files::sync_parent(&desired.unit)?;
        files::sync_parent(&desired.descriptor)?;
        Controller {
            layout,
            scope,
            runner,
        }
        .manager()
        .reload()?;
        return Ok(desired.clone());
    }
    let ctl = Controller {
        layout,
        scope,
        runner,
    };
    if !ctl.manager().available()? || ctl.manager().state(scope)? == State::Loaded {
        return Err(invalid("Cannot replace a loaded or unknown service unit."));
    }
    files::write(&desired.unit, desired.unit_contents.as_bytes(), true)?;
    files::write(&desired.descriptor, &files::encode(desired)?, true)?;
    ctl.manager().reload()?;
    Ok(desired.clone())
}
