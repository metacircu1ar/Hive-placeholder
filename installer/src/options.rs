pub(crate) mod input;
pub(crate) mod parsing;
use crate::Result;
use clap::Parser;
use std::{ffi::OsString, path::PathBuf};

#[derive(Parser)]
#[command(version, about = "Install verified Hive release parts and run setup")]
#[allow(clippy::struct_excessive_bools)]
pub struct Args {
    /// Directory containing verified native release assets.
    #[arg(long = "from", required_unless_present_any = ["uninstall", "staged_from"])]
    pub source: Option<PathBuf>,
    /// Verified bootstrap transport directory; keeps the user's original setup argv intact.
    #[arg(long, hide = true)]
    pub staged_from: Option<PathBuf>,
    /// Absolute installation directory; defaults to HOME/.local.
    #[arg(long)]
    pub prefix: Option<PathBuf>,
    /// Select `Hive` without its bundled knowledge integration.
    #[arg(long, conflicts_with = "standalone_comb")]
    pub core: bool,
    /// Add Comb, with text search and HTTP embeddings.
    #[arg(long)]
    pub comb: bool,
    /// Install only Comb; retain any existing `Hive` component.
    #[arg(long, conflicts_with = "core")]
    pub standalone_comb: bool,
    /// Install binaries without invoking their setup wizard.
    #[arg(long, conflicts_with = "uninstall")]
    pub no_setup: bool,
    /// Confirm interruption of owned services and the displayed uninstall scope.
    #[arg(long)]
    pub yes: bool,
    /// Remove selected installed commands and owned services, retaining user data.
    #[arg(long)]
    pub uninstall: bool,
    /// Preview removal without applying it.
    #[arg(long, requires = "uninstall")]
    pub dry_run: bool,
    /// Configuration file selected for purge.
    #[arg(long, requires = "purge")]
    pub config: Option<PathBuf>,
    /// Agent state directory selected for purge.
    #[arg(long, requires = "purge")]
    pub state_dir: Option<PathBuf>,
    /// Knowledge root whose managed integration is selected for purge.
    #[arg(long, requires = "purge")]
    pub comb_root: Option<PathBuf>,
    /// Bridge configuration selected for purge.
    #[arg(long, requires = "purge")]
    pub bridge_file: Option<PathBuf>,
    /// Also purge the displayed owned data; native provider history is retained.
    #[arg(long, requires = "uninstall")]
    pub purge: bool,
    /// Explicitly remove this selected knowledge root and its pages during purge.
    #[arg(long, requires = "purge")]
    pub purge_comb_root: Option<PathBuf>,
    /// Setup flags may follow --, or use --setup-FLAG before it.
    #[arg(last = true, conflicts_with_all = ["no_setup", "uninstall"])]
    pub setup: Vec<OsString>,
}
impl Args {
    pub fn read() -> std::result::Result<Self, hive_input::Rendered> {
        parsing::parse(forward(std::env::args_os().collect()))
    }
    pub fn prefix(&self) -> Result<PathBuf> {
        input::prefix(self.prefix.as_deref())
    }
    pub fn components(&self) -> Vec<String> {
        let mut result = vec![];
        if !self.standalone_comb {
            result.push("hive".into());
        }
        if self.comb || self.standalone_comb {
            result.push("comb".into());
        }
        result
    }
    pub fn parts(&self) -> Vec<&'static str> {
        let mut parts = vec![];
        if !self.standalone_comb {
            parts.push(if self.core { "hive-core" } else { "hive" });
        }
        if self.comb || self.standalone_comb {
            parts.push("comb");
        }
        parts
    }
}
fn forward(args: Vec<OsString>) -> Vec<OsString> {
    let mut install = vec![];
    let mut setup = vec![];
    let mut args = args.into_iter().peekable();
    while let Some(arg) = args.next() {
        if arg == "--" {
            setup.extend(args);
            break;
        }
        if matches!(
            arg.to_str(),
            Some(
                "--from"
                    | "--staged-from"
                    | "--prefix"
                    | "--config"
                    | "--state-dir"
                    | "--comb-root"
                    | "--bridge-file"
                    | "--purge-comb-root"
            )
        ) {
            install.push(arg);
            // Keep the adjacent token attached even when it is another flag.
            // Clap must diagnose the missing value before setup aliases move.
            if let Some(value) = args.next() {
                install.push(value);
            }
            continue;
        }
        if let Some(flag) = arg.to_str().and_then(|s| s.strip_prefix("--setup-")) {
            setup.push(format!("--{flag}").into());
            if !flag.contains('=')
                && args
                    .peek()
                    .is_some_and(|s| !s.to_string_lossy().starts_with('-'))
                && let Some(value) = args.next()
            {
                setup.push(value);
            }
        } else {
            install.push(arg);
        }
    }
    if !setup.is_empty() {
        install.push("--".into());
        install.extend(setup);
    }
    install
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn setup_arguments_keep_values_and_explicit_false() {
        let args = forward(
            [
                "install",
                "--from",
                "/assets",
                "--setup-provider",
                "codex,claude",
                "--setup-services=false",
                "--",
                "--login",
                "skip",
            ]
            .map(OsString::from)
            .into(),
        );
        let parsed = Args::try_parse_from(args).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            parsed.setup,
            [
                "--provider",
                "codex,claude",
                "--services=false",
                "--login",
                "skip"
            ]
            .map(OsString::from)
        );
    }
    #[test]
    fn plain_and_purge_removal_argv_parse_and_bootstrap_uses_staged_source() {
        let directory = tempfile::tempdir().unwrap();
        for purge in [false, true] {
            let mut args = vec![
                OsString::from("hive-install"),
                "--prefix".into(),
                directory.path().into(),
                "--uninstall".into(),
                "--dry-run".into(),
            ];
            if purge {
                args.extend(
                    [
                        "--purge",
                        "--config",
                        "/private/config",
                        "--state-dir",
                        "/private/state",
                        "--comb-root",
                        "/private/knowledge",
                        "--bridge-file",
                        "/private/bridge",
                    ]
                    .map(OsString::from),
                );
            }
            let parsed = parsing::parse(args).unwrap();
            assert_eq!(parsed.purge, purge);
            assert!(parsed.dry_run);
        }
        let parsed = parsing::parse(vec![
            "hive-install".into(),
            "--prefix".into(),
            directory.path().into(),
            "--staged-from".into(),
            directory.path().into(),
            "--from".into(),
            "https://assets.example/releases".into(),
            "--no-setup".into(),
        ])
        .unwrap();
        assert_eq!(
            parsed.source.as_deref(),
            Some(std::path::Path::new("https://assets.example/releases"))
        );
    }
    #[test]
    fn all_public_options_have_help() {
        use clap::CommandFactory;
        for argument in Args::command()
            .get_arguments()
            .filter(|arg| !arg.is_hide_set())
        {
            assert!(argument.get_help().is_some(), "{}", argument.get_id());
        }
    }
}
