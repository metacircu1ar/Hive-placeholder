//! Freeze native purge receipts before uninstall; never roll data deletion back.
mod images;
mod jobs;
mod services;
use crate::{
    Result, lifecycle::Lifecycle as _, manifest, options::Args, publish, services::Services,
    uninstall,
};
use hive_setup_support::{
    files,
    maintenance::{Plan, Scope},
    removal::{self, Action},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
const RECEIPT: &str = ".hive-purge.json";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    component: String,
    binary: PathBuf,
    receipt: PathBuf,
    plan: Plan<serde_json::Value>,
    done: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    format: crate::compatibility::Format,
    prefix: PathBuf,
    components: Vec<String>,
    scope: Scope,
    purge_root: Option<PathBuf>,
    jobs: Vec<Job>,
    services: services::Cleanup,
    image: Action,
    images_admitted: bool,
    uninstalled: bool,
    active: usize,
    complete: bool,
}
impl Receipt {
    fn save(&self, path: &Path) -> Result<()> {
        Ok(files::write(path, &files::encode(self)?, true)?)
    }
    fn prepare_progress(&self) -> Result<()> {
        // Child progress exists durably before any discovery input disappears.
        for job in &self.jobs {
            if files::read(&job.receipt)?.is_none() {
                if self.uninstalled {
                    return Err(
                        "Native purge progress is missing after uninstall; refusing to \
                        reset its receipt"
                            .into(),
                    );
                }
                files::write(&job.receipt, &files::encode(&job.plan)?, false)?;
            }
        }
        Ok(())
    }
    fn validate(&self, prefix: &Path) -> Result<()> {
        if self.prefix != prefix
            || self.jobs.len() > 2
            || self.components.is_empty()
            || self.components.len() > 2
            || self
                .components
                .iter()
                .any(|s| !["hive", "comb"].contains(&s.as_str()))
            || self.image.object.path != prefix.join("lib/hive")
        {
            return Err("Unsupported purge receipt".into());
        }
        self.image.validate()?;
        self.services.validate(prefix, &self.components)?;
        validate_scope(&prefix.join("lib/hive"), &self.scope)?;
        for job in &self.jobs {
            if !["hive", "comb"].contains(&job.component.as_str())
                || job.receipt != prefix.join(format!("lib/.hive-purge-{}.json", job.component))
            {
                return Err("Invalid native maintenance receipt path".into());
            }
        }
        Ok(())
    }
}
pub fn run(args: &Args) -> Result<()> {
    let prefix = args.prefix()?;
    let manager = prefix.join("lib/hive");
    let _lock = publish::installation_lock(&manager)?;
    let path = prefix.join("lib").join(RECEIPT);
    let mut services = Services::new(true)?;
    let existing: Option<Receipt> = files::json(&path)?;
    let mut receipt = if let Some(receipt) = existing {
        receipt.validate(&prefix)?;
        jobs::check_options(args, &receipt)?;
        receipt
    } else {
        if !args.dry_run {
            services.recover(&prefix, &manager)?;
        } else if manager.join("pending-install.json").exists() {
            return Err("Recover pending installation before purge preview".into());
        }
        prepare(&prefix, args, &services)?
    };
    preview(&receipt)?;
    if args.dry_run {
        return Ok(());
    }
    if receipt.complete {
        println!("Purge is already complete; no objects were deleted again.");
        return Ok(());
    }
    crate::services::confirm(
        args.yes,
        "Permanently purge exactly the displayed product data after uninstall?",
    )?;
    tracing::info!(name: "install.purge_started", count = receipt.jobs.len());
    receipt.save(&path)?;
    receipt.prepare_progress()?;
    if !receipt.uninstalled {
        let removed =
            uninstall::remove_locked(&prefix, &receipt.components, false, &mut services, |_| {
                Ok(())
            })?;
        receipt.active = removed.active;
        receipt.uninstalled = true;
        receipt.save(&path)?;
    }
    for index in 0..receipt.jobs.len() {
        if receipt.jobs[index].done {
            continue;
        }
        jobs::verify_binary(&manager, &receipt.jobs[index])?;
        jobs::apply(&receipt.jobs[index]).map_err(|error| {
            crate::publication::context(
                "Uninstall committed; native purge is incomplete. Retry the same \
                --uninstall --purge command",
                error,
            )
        })?;
        receipt.jobs[index].done = true;
        receipt.save(&path)?;
    }
    for index in 0..receipt.services.len() {
        receipt.services.remove(
            index,
            &hive_setup_support::runner::SystemRunner(std::process::Command::spawn),
        )?;
        receipt.save(&path)?;
    }
    if receipt.active == 0 {
        if !receipt.images_admitted {
            images::validate(&receipt.image.object.path)?;
            receipt.images_admitted = true;
            receipt.save(&path)?;
        }
        images::remove(&mut receipt.image)?;
    }
    receipt.complete = true;
    receipt.save(&path)?;
    tracing::info!(name: "install.purge_finished", count = receipt.jobs.len(), success = true);
    println!(
        "Purge completed. Native login/history state and the displayed retained paths remain."
    );
    Ok(())
}
fn prepare(prefix: &Path, args: &Args, services: &Services) -> Result<Receipt> {
    let manager = prefix.join("lib/hive");
    let pointer = publish::current(&manager)?.ok_or("Missing installation image")?;
    let installed = publish::load(&manager, Some(&pointer), &manifest::native_target()?)?;
    let components = args.components();
    let mut binaries = BTreeMap::new();
    for component in ["hive", "comb"] {
        let generation = if installed.parts.contains_key(component) {
            Some(&pointer)
        } else {
            installed.retired.get(component)
        };
        if let Some(generation) = generation {
            let binary = manifest::maintenance_binary(component);
            binaries.insert(
                component.to_owned(),
                manager.join(generation).join("bin").join(binary),
            );
        }
    }
    let (jobs, scope) = jobs::prepare(prefix, args, &binaries)?;
    let (layout, specs) = services.removal_specs(prefix, &components)?;
    let services = services::Cleanup::capture(layout, specs)?;
    validate_scope(&manager, &scope)?;
    let purge_root = args
        .purge_comb_root
        .as_deref()
        .map(removal::safe_path)
        .transpose()?;
    Ok(Receipt {
        format: hive_input::compatibility::Exact,
        prefix: prefix.into(),
        components,
        scope,
        purge_root,
        jobs,
        services,
        image: Action::capture(&manager, "images")?,
        images_admitted: false,
        uninstalled: false,
        active: installed.parts.len(),
        complete: false,
    })
}
fn preview(receipt: &Receipt) -> Result<()> {
    receipt.services.preview();
    eprintln!(
        "Purge scope: {}",
        serde_json::to_string_pretty(&receipt.scope)?
    );
    for job in &receipt.jobs {
        eprintln!(
            "{} native cleanup: {}",
            job.plan.product,
            serde_json::to_string_pretty(&job.plan)?
        );
    }
    eprintln!(
        "Knowledge root deletion: {:?}. Native provider history, user skill copies \
            and backups are retained.",
        receipt.purge_root
    );
    eprintln!(
        "When no active component remains, remove validated installation \
            generations at {} after checking live executable references.",
        receipt.image.object.path.display()
    );
    Ok(())
}
pub(crate) fn fence_install(prefix: &Path) -> Result<()> {
    let path = prefix.join("lib").join(RECEIPT);
    if let Some(receipt) = files::json::<Receipt>(&path)? {
        receipt.validate(prefix)?;
        if !receipt.complete {
            return Err(
                "Purge is pending for this prefix; finish --uninstall --purge before reinstalling"
                    .into(),
            );
        }
        for job in receipt.jobs {
            if let Some(bytes) = files::read(&job.receipt)? {
                files::remove(&job.receipt, &bytes)?;
            }
        }
        let bytes = files::read(&path)?.ok_or("Purge receipt disappeared")?;
        files::remove(&path, &bytes)?;
    }
    Ok(())
}

fn validate_scope(manager: &Path, scope: &Scope) -> Result<()> {
    let paths: Vec<&Path> = [&scope.config, &scope.state, &scope.root, &scope.bridge]
        .into_iter()
        .flatten()
        .map(PathBuf::as_path)
        .collect();
    removal::disjoint(&paths)?;
    for path in paths {
        removal::disjoint(&[manager, path])?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn cross_product_overlap_refuses_before_any_deletion() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let knowledge = state.join("runtime/output/knowledge");
        std::fs::create_dir_all(&knowledge).unwrap();
        let page = knowledge.join("fact.md");
        std::fs::write(&page, b"keep").unwrap();
        let mut scope = Scope {
            state: Some(state),
            root: Some(knowledge.clone()),
            ..Scope::default()
        };
        assert!(validate_scope(&temp.path().join("installation"), &scope).is_err());
        scope.root = None;
        scope.bridge = Some(knowledge.join("bridge.json"));
        assert!(validate_scope(&temp.path().join("installation"), &scope).is_err());
        assert_eq!(std::fs::read(page).unwrap(), b"keep");
    }
}
