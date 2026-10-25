//! Exhaustive option classification before uninstall helper execution.
use super::{Result, Values};
use crate::cli::input::Context;
use hive_input::Code;

#[derive(Clone, Copy)]
enum Use {
    Control,
    Scope,
    Wizard,
}

pub(super) fn values(value: &Values) -> Result<()> {
    for (flag, supplied, usage) in fields(value) {
        if supplied {
            check(flag, usage, value.purge == Some(true))?;
        }
    }
    Ok(())
}

fn check(flag: &'static str, usage: Use, purge: bool) -> Result<()> {
    let expected = match usage {
        Use::Control => return Ok(()),
        Use::Scope if purge => return Ok(()),
        Use::Scope => "use --uninstall --purge, or omit this scope option",
        Use::Wizard => "omit this setup option with --uninstall",
    };
    Err(invalid(
        flag,
        "Option is not used by this removal mode.",
        expected,
    ))
}

pub(super) fn invalid(
    flag: &'static str,
    message: &'static str,
    expected: &str,
) -> crate::error::Error {
    Context::new("setup", flag)
        .invalid(Code::ConflictingArguments, message, expected)
        .error
}

fn fields(value: &Values) -> impl Iterator<Item = (&'static str, bool, Use)> {
    // Destructuring without `..` makes a new Values field a compile-time omission.
    macro_rules! classify {
        ($($usage:ident { $($field:ident => $flag:literal),* $(,)? }),* $(,)?) => {{
            let Values { $($($field,)*)* } = value;
            [ $($(($flag, $field.is_some(), Use::$usage),)*)* ]
        }};
    }
    classify!(
        Control {
            uninstall => "--uninstall", purge => "--purge", prefix => "--prefix",
            yes => "--yes", dry_run => "--dry-run",
        },
        Scope {
            purge_comb_root => "--purge-comb-root", comb_root => "--comb-root",
            bridge_file => "--bridge-file",
        },
        Wizard {
            provider => "--provider", default_provider => "--default-provider",
            login => "--login", codex_profile => "--codex-profile",
            claude_profile => "--claude-profile", codex_model => "--codex-model",
            claude_model => "--claude-model", codex_effort => "--codex-effort",
            claude_effort => "--claude-effort",
            embedding_config => "--embedding-config", comb_bin => "--comb-bin",
            no_comb => "--no-comb", no_comb_roles => "--no-comb-roles",
            collectors => "--collectors", user_skills => "--user-skills",
            skills_dir => "--skills-dir", install_provider => "--install-provider",
            services => "--services", no_services => "--no-services",
            restart_daemon => "--restart-daemon",
            preserve_stopped_services => "--preserve-stopped-services",
            service_environment => "--service-environment", service_path => "--service-path",
        },
    )
    .into_iter()
}
