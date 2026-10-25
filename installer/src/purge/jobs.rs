use super::{Args, Job, Plan, Receipt, Result, Scope, manifest, publish, removal};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(super) fn prepare(
    prefix: &Path,
    args: &Args,
    binaries: &BTreeMap<String, PathBuf>,
) -> Result<(Vec<Job>, Scope)> {
    let mut jobs = vec![];
    let mut scope = Scope::default();
    if args.components().iter().any(|s| s == "hive")
        && let Some(binary) = binaries.get("hive")
    {
        let mut command = vec![];
        push(&mut command, "--config", args.config.as_deref())?;
        push(&mut command, "--state-dir", args.state_dir.as_deref())?;
        command.push("setup-maintenance".into());
        scope_args(
            &mut command,
            args.comb_root.as_deref(),
            args.bridge_file.as_deref(),
        )?;
        let plan = call(binary, &command)?;
        scope = plan.scope.clone();
        jobs.push(job(prefix, "hive", binary, plan));
    }
    let detach = args.components().iter().any(|s| s == "comb");
    if detach || scope.bridge.is_some() || args.bridge_file.is_some() {
        let binary = binaries.get("comb").ok_or(
            "Comb maintenance binary is required to verify and revoke this bridge; \
            retain it or install Comb before purge",
        )?;
        let mut command = vec!["setup-maintenance".into()];
        scope_args(
            &mut command,
            args.comb_root.as_deref().or(scope.root.as_deref()),
            args.bridge_file.as_deref().or(scope.bridge.as_deref()),
        )?;
        if detach {
            command.push("--detach-comb".into());
        }
        push(
            &mut command,
            "--purge-comb-root",
            args.purge_comb_root.as_deref(),
        )?;
        let plan = call(binary, &command)?;
        scope.root.clone_from(&plan.scope.root);
        scope.bridge.clone_from(&plan.scope.bridge);
        let job = job(prefix, "comb", binary, plan);
        // A retained, possibly serving Comb must revoke before Hive config goes away.
        if detach {
            jobs.push(job);
        } else {
            jobs.insert(0, job);
        }
    }
    for job in &jobs {
        if hive_setup_support::files::optional_metadata(&job.receipt)?.is_some() {
            return Err(
                "Orphan native purge receipt exists; inspect it before admitting a new purge"
                    .into(),
            );
        }
    }
    Ok((jobs, scope))
}
fn job(prefix: &Path, component: &str, binary: &Path, plan: Plan<serde_json::Value>) -> Job {
    Job {
        component: component.into(),
        binary: binary.into(),
        receipt: prefix.join(format!("lib/.hive-purge-{component}.json")),
        plan,
        done: false,
    }
}
fn scope_args(args: &mut Vec<String>, root: Option<&Path>, bridge: Option<&Path>) -> Result<()> {
    push(args, "--knowledge-root", root)?;
    push(args, "--bridge-file", bridge)?;
    Ok(())
}
fn push(args: &mut Vec<String>, flag: &str, path: Option<&Path>) -> Result<()> {
    if let Some(path) = path {
        args.extend([flag.into(), hive_setup_support::service::path_text(path)?]);
    }
    Ok(())
}
fn call(binary: &Path, args: &[String]) -> Result<Plan<serde_json::Value>> {
    let output = hive_setup_support::runner::progress(
        std::process::Command::spawn,
        binary.to_str().ok_or("Invalid native program path")?,
        args,
        &|| false,
    )?;
    if output.code != 0 {
        return Err(format!("Native maintenance failed: {}", output.stdout).into());
    }
    Ok(serde_json::from_str(&output.stdout)?)
}
pub(super) fn apply(job: &Job) -> Result<()> {
    let result = call(
        &job.binary,
        &[
            "setup-maintenance".into(),
            "--apply".into(),
            hive_setup_support::service::path_text(&job.receipt)?,
        ],
    )?;
    if result.product != job.plan.product
        || result.scope != job.plan.scope
        || result.removals.iter().any(|a| !a.done)
    {
        return Err("Native cleanup did not confirm its exact admitted scope".into());
    }
    Ok(())
}
pub(super) fn verify_binary(manager: &Path, job: &Job) -> Result<()> {
    let relative = job.binary.strip_prefix(manager)?;
    let parts: Vec<_> = relative.components().collect();
    if parts.len() != 4
        || parts[2].as_os_str() != "bin"
        || parts[3].as_os_str() != manifest::maintenance_binary(&job.component)
    {
        return Err("Invalid retained maintenance executable".into());
    }
    let pointer = PathBuf::from(parts[0].as_os_str()).join(parts[1].as_os_str());
    publish::retired_part(
        manager,
        &pointer,
        &job.component,
        &manifest::native_target()?,
    )?;
    Ok(())
}
pub(super) fn check_options(args: &Args, receipt: &Receipt) -> Result<()> {
    if args.components() != receipt.components {
        return Err("Purge retry must select the original components".into());
    }
    for (flag, expected) in [
        (&args.config, &receipt.scope.config),
        (&args.state_dir, &receipt.scope.state),
        (&args.comb_root, &receipt.scope.root),
        (&args.bridge_file, &receipt.scope.bridge),
        (&args.purge_comb_root, &receipt.purge_root),
    ] {
        if let Some(path) = flag
            && Some(removal::safe_path(path)?) != *expected
        {
            return Err(
                "Purge flags conflict with the durable original scope; use its recorded paths"
                    .into(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;
    #[test]
    fn deletion_paths_never_become_a_lossy_sibling() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp
            .path()
            .join(std::ffi::OsString::from_vec(b"scope-\xff".to_vec()));
        let sibling = temp.path().join("scope-\u{fffd}");
        // APFS rejects invalid UTF-8 filenames; Linux permits them.
        #[cfg(target_os = "linux")]
        std::fs::write(&path, b"original").unwrap();
        std::fs::write(&sibling, b"unrelated").unwrap();
        assert!(push(&mut vec![], "--bridge-file", Some(&path)).is_err());
        #[cfg(target_os = "linux")]
        assert_eq!(std::fs::read(path).unwrap(), b"original");
        assert_eq!(std::fs::read(sibling).unwrap(), b"unrelated");
    }

    #[test]
    fn retired_components_use_their_cli_and_reject_other_owned_binaries() -> Result<()> {
        let root = tempfile::tempdir()?;
        let prefix = root.path().join("prefix");
        let parts = [
            crate::tests::fixture(root.path(), "hive"),
            crate::tests::fixture(root.path(), "comb"),
        ];
        publish::install(&prefix, root.path(), &parts, |_| Ok(()))?;
        let removed = crate::uninstall::remove_with(
            &prefix,
            &["hive".into(), "comb".into()],
            true,
            false,
            &mut crate::lifecycle::Inert,
            |_| Ok(()),
        )?;
        assert_eq!(removed.binaries.len(), 2);
        let manager = prefix.join("lib/hive");
        for (component, binary) in removed.binaries {
            let mut retained = job(
                &prefix,
                &component,
                &binary,
                Plan {
                    format: hive_input::compatibility::Exact,
                    product: component.clone(),
                    scope: Scope::default(),
                    retained: vec![],
                    removals: vec![],
                    details: serde_json::Value::Null,
                },
            );
            verify_binary(&manager, &retained)?;
            if component == "hive" {
                for other in ["hive", "hive-daemon"] {
                    retained.binary.set_file_name(other);
                    assert!(verify_binary(&manager, &retained).is_err());
                }
            }
        }
        Ok(())
    }
}
