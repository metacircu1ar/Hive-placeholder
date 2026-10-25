//! Caller selection errors are identified before native authentication starts.
use crate::{
    cli::{AuthCommand, input::Context},
    config::{Config, Method, Profile, Provider},
    error::{Error, Result},
    state::State,
};
use hive_input::Code;
use std::path::Path;

pub(in crate::cli) fn profile<'a>(
    config: &'a Config,
    name: &str,
    command: &'static str,
    field: &'static str,
) -> Result<&'a Profile> {
    config.auth_profiles.get(name).ok_or_else(|| {
        invalid(
            command,
            field,
            "No configured login profile has this name.",
            "a profile name from auth list in the selected configuration",
        )
    })
}

pub(in crate::cli) fn models(profile: &Profile) -> Result<()> {
    profile.validate()
}

pub(in crate::cli) fn steering(profile: &Profile) -> Result<()> {
    if profile.provider != Provider::Claude || profile.method != Method::Subscription {
        return Err(invalid(
            "verify-steering",
            "--auth",
            "Steering verification requires a Claude subscription profile.",
            "a configured Claude subscription profile",
        ));
    }
    Ok(())
}

pub(super) fn auth(config: &Config, command: &AuthCommand, root: &Path) -> Result<()> {
    let (key, name) = match command {
        AuthCommand::Status { name } => ("auth status", name),
        AuthCommand::Login { name, .. } => ("auth login", name),
        AuthCommand::Recover { name } => ("auth recover", name),
        AuthCommand::Repair { name, .. } => ("auth repair", name),
        _ => return Ok(()),
    };
    let profile = profile(config, name, key, "NAME")?;
    let result = match command {
        AuthCommand::Login {
            browser,
            reauthenticate,
            ..
        } => login(profile, *browser, *reauthenticate),
        AuthCommand::Repair { browser, .. } => {
            subscription(profile, key, "NAME").and_then(|()| browser_option(profile, key, *browser))
        }
        _ => Ok(()),
    };
    if result.is_err()
        && matches!(
            command,
            AuthCommand::Repair {
                new_state_dir: None,
                ..
            }
        )
    {
        // In-place repair must retain its corruption/ownership guard before
        // rejecting the method or browser choice. Valid repair checks it itself.
        State::new(root.to_path_buf())?.check_format()?;
    }
    result
}

fn login(profile: &Profile, browser: bool, reauthenticate: bool) -> Result<()> {
    browser_option(profile, "auth login", browser)?;
    if reauthenticate {
        subscription(profile, "auth login", "--reauthenticate")?;
    }
    Ok(())
}

fn browser_option(profile: &Profile, command: &'static str, browser: bool) -> Result<()> {
    if browser && (profile.provider != Provider::Codex || profile.method != Method::Subscription) {
        return Err(invalid(
            command,
            "--browser",
            "Browser login requires a Codex subscription profile.",
            "--browser with a Codex subscription profile, or omit --browser",
        ));
    }
    Ok(())
}

fn subscription(profile: &Profile, command: &'static str, field: &'static str) -> Result<()> {
    if profile.method != Method::Subscription {
        return Err(invalid(
            command,
            field,
            "This operation requires a subscription profile.",
            "a configured subscription profile",
        ));
    }
    Ok(())
}

fn invalid(
    command: &'static str,
    field: &'static str,
    message: &'static str,
    expected: &'static str,
) -> Error {
    Context::new(command, field)
        .invalid(Code::InvalidValue, message, expected)
        .error
}
