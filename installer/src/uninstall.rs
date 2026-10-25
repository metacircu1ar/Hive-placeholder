//! Removing active entrypoints is a journaled publication, not generation GC.
use crate::{
    Result,
    lifecycle::{Change, Lifecycle, Operation},
    manifest, publish, services,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub struct Removed {
    pub binaries: BTreeMap<String, PathBuf>,
    pub active: usize,
}
pub fn run(prefix: &Path, components: &[String], yes: bool, dry_run: bool) -> Result<Removed> {
    let mut lifecycle = services::Services::new(yes)?;
    remove_with(prefix, components, yes, dry_run, &mut lifecycle, |_| Ok(()))
}
pub(crate) fn remove_with(
    prefix: &Path,
    components: &[String],
    _yes: bool,
    dry_run: bool,
    lifecycle: &mut dyn Lifecycle,
    hook: impl FnMut(&str) -> Result<()>,
) -> Result<Removed> {
    let manager = prefix.join("lib/hive");
    if !manager.try_exists()? {
        return Err(format!("No managed installation at {}", manager.display()).into());
    }
    let _lock = publish::installation_lock(&manager)?;
    remove_locked(prefix, components, dry_run, lifecycle, hook)
}
pub(crate) fn remove_locked(
    prefix: &Path,
    components: &[String],
    dry_run: bool,
    lifecycle: &mut dyn Lifecycle,
    mut hook: impl FnMut(&str) -> Result<()>,
) -> Result<Removed> {
    let manager = prefix.join("lib/hive");
    publish::manager(prefix, &mut hook)?;
    if dry_run {
        if manager.join("pending-install.json").try_exists()? {
            return Err(
                "Pending installation needs recovery before a read-only uninstall preview".into(),
            );
        }
    } else {
        lifecycle.recover(prefix, &manager)?;
    }
    lifecycle.preview_removal(prefix, components)?;
    let old = publish::current(&manager)?.ok_or("Installation has no current image")?;
    let mut installed = publish::load(&manager, Some(&old), &manifest::native_target()?)?;
    let old_links = publish::links(&installed.parts)?;
    if !publish::check_links(prefix, &old_links)?.is_empty() {
        return Err("Owned links are missing; refusing uninstall".into());
    }
    let mut binaries = BTreeMap::new();
    for component in components {
        if installed.parts.remove(component).is_some() {
            installed.retired.insert(component.clone(), old.clone());
        }
        if let Some(pointer) = installed.retired.get(component) {
            let binary = manifest::maintenance_binary(component);
            binaries.insert(
                component.clone(),
                manager.join(pointer).join("bin").join(binary),
            );
        }
    }
    let remaining = publish::links(&installed.parts)?;
    let removed: BTreeMap<_, _> = old_links
        .into_iter()
        .filter(|(name, _)| !remaining.contains_key(name))
        .collect();
    for name in removed.keys() {
        eprintln!("Remove owned entrypoint: {}", prefix.join(name).display());
    }
    eprintln!(
        "Retain configuration, state, native provider histories and Comb roots \
            unless --purge is explicit."
    );
    eprintln!(
        "Retain immutable images at {} (partial uninstall is not image pruning).",
        manager.join("generations").display()
    );
    let result = Removed {
        binaries,
        active: installed.parts.len(),
    };
    if dry_run || removed.is_empty() {
        return Ok(result);
    }
    tracing::info!(name: "install.uninstall_started", count = components.len());

    let generation =
        publish::stage_generation(&manager, Path::new(""), &mut installed, &[], Some(&old))?;
    let change = Change {
        operation: Operation::Uninstall,
        prefix: prefix.into(),
        manager: manager.clone(),
        previous: Some(old),
        next: generation.strip_prefix(&manager)?.into(),
        components: components.to_vec(),
        added_links: BTreeMap::new(),
        removed_links: removed,
    };
    if let Err(error) = lifecycle
        .prepare(&change)
        .and_then(|()| execute(&change, lifecycle, &mut hook))
    {
        let cleanup = crate::publication::discard_unreferenced(&manager, &generation);
        return Err(crate::publication::failed(
            error,
            "Uninstall cleanup failed",
            cleanup,
        ));
    }
    tracing::info!(name: "install.uninstall_finished", count = result.active, success = true);
    Ok(result)
}
fn execute(
    change: &Change,
    lifecycle: &mut dyn Lifecycle,
    hook: &mut impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    if let Err(error) = (|| {
        lifecycle.stop()?;
        hook("before_replace")?;
        publish::replace(&change.manager, &change.next)?;
        hook("after_replace")?;
        publish::sync(&change.manager)?;
        services::reconcile_removed(&change.prefix, &change.removed_links, false)?;
        hook("after_links")?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    })() {
        // Once published, finish this uninstall on retry; never claim rollback.
        return Err(failed(change, lifecycle, error));
    }
    lifecycle.finish(true).map_err(|error| {
        crate::publication::context("Uninstall committed; cleanup needs recovery", error)
    })
}

fn failed(
    change: &Change,
    lifecycle: &mut dyn Lifecycle,
    error: Box<dyn std::error::Error + Send + Sync>,
) -> Box<dyn std::error::Error + Send + Sync> {
    let current = match publish::current(&change.manager) {
        Ok(current) => current,
        Err(secondary) => {
            return crate::publication::failed(
                error,
                "Uninstall publication status is uncertain",
                Err(secondary),
            );
        }
    };
    if current.as_ref() == Some(&change.next) {
        tracing::warn!(name: "install.uninstall_pending", decision = "finish_on_retry");
        return crate::publication::context(
            "Uninstall is committed but incomplete; retry to finish owned link/service removal",
            error,
        );
    }
    let restore = services::reconcile_removed(&change.prefix, &change.removed_links, true)
        .and_then(|()| lifecycle.finish(false));
    crate::publication::failed(error, "Previous services need recovery", restore)
}
