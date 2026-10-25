//! Replace the CLI with the native helper so no parent image pins a full purge.
use super::{CommandOutcome, Path, Result, invalid, io, options::Values, parts};
use std::{os::unix::process::CommandExt as _, process::Command};
pub(super) fn run(config: &Path, root: &Path, options: &Values) -> Result<CommandOutcome> {
    let prefix = match &options.prefix {
        Some(path) => hive_setup_support::files::absolute(path).map_err(io)?,
        None => parts::invoked()?
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| invalid("Cannot infer installation prefix; pass --prefix."))?
            .to_owned(),
    };
    let helper = prefix.join("bin/hive-install");
    if !helper.is_file() {
        return Err(invalid(format!(
            "No installed helper at {}; use the release install.sh --uninstall --prefix {}.",
            helper.display(),
            prefix.display()
        )));
    }
    let mut command = Command::new(&helper);
    arguments(&mut command, &prefix, config, root, options);
    Err(io(command.exec()))
}
fn arguments(command: &mut Command, prefix: &Path, config: &Path, root: &Path, options: &Values) {
    command.args(["--uninstall", "--prefix"]).arg(prefix);
    for (flag, path) in [
        ("--config", Some(config)),
        ("--state-dir", Some(root)),
        ("--comb-root", options.comb_root.as_deref()),
        ("--bridge-file", options.bridge_file.as_deref()),
        ("--purge-comb-root", options.purge_comb_root.as_deref()),
    ] {
        if options.purge == Some(true)
            && let Some(path) = path
        {
            command.arg(flag).arg(path);
        }
    }
    for (flag, value) in [
        ("--yes", options.yes),
        ("--dry-run", options.dry_run),
        ("--purge", options.purge),
    ] {
        if value == Some(true) {
            command.arg(flag);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scope_is_forwarded_only_for_purge_while_dry_run_works_for_both() {
        for purge in [false, true] {
            let mut command = Command::new("hive-install");
            let values = Values {
                purge: Some(purge),
                dry_run: Some(true),
                comb_root: Some("/private/knowledge".into()),
                bridge_file: Some("/private/bridge".into()),
                ..Values::default()
            };
            arguments(
                &mut command,
                Path::new("/private/prefix"),
                Path::new("/private/config"),
                Path::new("/private/state"),
                &values,
            );
            let args: Vec<_> = command.get_args().collect();
            for flag in [
                "--config",
                "--state-dir",
                "--comb-root",
                "--bridge-file",
                "--purge",
            ] {
                assert_eq!(args.iter().any(|arg| *arg == flag), purge, "{flag}");
            }
            assert!(args.iter().any(|arg| *arg == "--dry-run"));
        }
    }
}
