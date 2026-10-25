//! Reuse native login ownership; select only models and efforts actually advertised.
use super::{
    Result, invalid, io,
    options::{Login, Values},
    plan::Plan,
    publication::Snapshot,
    terminal,
};
use crate::{
    auth::{Auth, Settings, Status},
    config::Provider,
    process::Cancellation,
    state::State,
};
use serde::Deserialize;
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command, time::Duration};

pub(super) fn configure(
    cli: &super::super::Cli,
    root: &Path,
    snapshot: &Snapshot,
    plan: &mut Plan,
    options: &Values,
) -> Result<Vec<String>> {
    let candidate = plan.candidate(snapshot)?;
    let mut pending = Vec::new();
    let command = super::super::doctor::command(cli, snapshot.target(), root, "auth login");
    for selected in &mut plan.providers {
        let binary = install_binary(cli, selected.provider, options)?;
        if let Err(error) = &binary
            && plan.login != Login::Skip
        {
            return Err(invalid(format!(
                "{} CLI is missing. Run `npm install -g {}` or pass --install-provider {}.",
                selected.provider.as_str(),
                package(selected.provider),
                selected.provider.as_str()
            ))
            .with_source(error.clone()));
        }
        let auth = authentication(cli, root, &candidate, plan.login)?;
        let mut report = auth.status(&selected.profile)?;
        if report.state != Status::Authenticated && plan.login != Login::Skip {
            login(
                cli,
                root,
                &candidate,
                &selected.profile,
                selected.provider,
                plan.login,
            )?;
            report = auth.status(&selected.profile)?;
        }
        if report.state == Status::Authenticated {
            let value =
                crate::runtime::models(&candidate, &auth.state, &selected.profile, &binary?)?;
            let catalog: Catalog = serde_json::from_value(value)
                .map_err(|cause| invalid("Invalid provider catalog.").with_source(cause))?;
            let (model, effort) =
                catalog.select(selected.model.as_deref(), selected.effort.as_deref())?;
            eprintln!(
                "{}: {model}, effort {effort} ({})",
                selected.provider.as_str(),
                selected.profile
            );
            selected.model = Some(model);
            selected.effort = Some(effort);
            selected.verified = true;
        } else if plan.login == Login::Skip
            && matches!(
                report.state,
                Status::Missing | Status::ReauthenticationRequired | Status::PresentUnverified
            )
        {
            pending_login(selected, &binary, &command, &mut pending)?;
        } else {
            return Err(invalid(format!(
                "Profile {} cannot be verified ({:?}). Resolve auth status/recovery, then \
                    rerun setup.",
                selected.profile, report.state
            )));
        }
    }
    plan.complete()?;
    Ok(pending)
}
fn package(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => "@openai/codex",
        Provider::Claude => "@anthropic-ai/claude-code",
    }
}
#[derive(Deserialize)]
struct Catalog {
    data: Vec<Entry>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    model: String,
    #[serde(default)]
    is_default: bool,
    default_reasoning_effort: Option<String>,
    supported_reasoning_efforts: Vec<Effort>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Effort {
    reasoning_effort: String,
}
impl Catalog {
    fn select(&self, model: Option<&str>, effort: Option<&str>) -> Result<(String, String)> {
        let entry = match model {
            Some(name) => self.data.iter().find(|m| m.model == name),
            None => self
                .data
                .iter()
                .find(|m| m.is_default)
                .or(self.data.first()),
        }
        .ok_or_else(|| {
            invalid(
                "Selected model is not advertised by the provider; run hivectl models --auth \
                    PROFILE.",
            )
        })?;
        let effort = effort
            .or(entry.default_reasoning_effort.as_deref())
            .or_else(|| {
                entry
                    .supported_reasoning_efforts
                    .first()
                    .map(|e| e.reasoning_effort.as_str())
            })
            .filter(|wanted| {
                entry
                    .supported_reasoning_efforts
                    .iter()
                    .any(|e| e.reasoning_effort == *wanted)
            })
            .ok_or_else(|| invalid("Selected effort is not advertised for this model."))?;
        Ok((entry.model.clone(), effort.into()))
    }
}

fn login(
    cli: &super::super::Cli,
    root: &Path,
    candidate: &crate::config::Config,
    profile: &str,
    provider: Provider,
    login: Login,
) -> Result<()> {
    if !terminal::Terminal::open(false).available() {
        return Err(invalid(
            "Native login needs a controlling terminal; use --login skip with explicit \
                model/effort for unattended setup.",
        ));
    }
    // Existing auth CLI retains its terminal relay and recovery guarantees. A
    // private ephemeral profiles-only config avoids changing the user's file twice.
    let yaml = serde_yaml_ng::to_string(&crate::runtime::api::value(LoginConfiguration {
        auth_profiles: &candidate.auth_profiles,
    }))
    .map_err(|cause| invalid("Cannot encode login configuration.").with_source(cause))?;
    let (temporary, config) = login_configuration(yaml.as_bytes())?;
    let mut child = Command::new(crate::executable::path()?);
    child
        .arg("--config")
        .arg(&config)
        .arg("--state-dir")
        .arg(root)
        .arg("--codex-bin")
        .arg(&cli.codex_bin)
        .arg("--claude-bin")
        .arg(&cli.claude_bin)
        .arg("--probe-timeout")
        .arg(cli.probe_timeout.to_string())
        .arg("--login-timeout")
        .arg(cli.login_timeout.to_string())
        .args(["auth", "login"])
        .arg(profile);
    if provider == Provider::Codex && login == Login::Browser {
        child.arg("--browser");
    }
    terminal::interactive(&mut child)?;
    drop(temporary);
    Ok(())
}

fn login_configuration(bytes: &[u8]) -> Result<(tempfile::TempDir, std::path::PathBuf)> {
    let directory = tempfile::Builder::new()
        .prefix(".hive-tmp-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .map_err(|cause| {
            crate::error::Error::state_io("Cannot prepare native login configuration.")
                .with_source(cause)
        })?;
    crate::publication::prepare(directory, |root| {
        let path = root.join("profiles.yaml");
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .create_new(&path, bytes)
            .map_err(crate::publication::error)?;
        Ok(path)
    })
}

fn authentication(
    cli: &super::super::Cli,
    root: &Path,
    candidate: &crate::config::Config,
    login: Login,
) -> Result<Auth> {
    let auth = Auth {
        config: candidate.clone(),
        state: State::new(root.into())?,
        settings: Settings {
            launcher: crate::launch::system_cache(),
            codex: cli.codex_bin.clone(),
            claude: cli.claude_bin.clone(),
            probe_timeout: Duration::from_secs(cli.probe_timeout),
            login_timeout: Duration::from_secs(cli.login_timeout),
            browser: login == Login::Browser,
            reauthenticate: false,
            cancellation: Cancellation::install()?,
        },
    };
    Ok(auth)
}

fn install_binary(
    cli: &super::super::Cli,
    provider: Provider,
    options: &Values,
) -> Result<Result<std::path::PathBuf>> {
    let requested = match provider {
        Provider::Codex => &cli.codex_bin,
        Provider::Claude => &cli.claude_bin,
    };
    let mut binary = crate::process::executable(requested).inspect_err(|error| {
        crate::observability::observation_failed("setup_provider_lookup", error);
    });
    if binary.is_err()
        && options
            .install_provider
            .as_ref()
            .is_some_and(|p| p.contains(&provider))
    {
        if terminal::Terminal::open(false).available() {
            let mut cmd = Command::new("npm");
            cmd.args(["install", "-g", package(provider)]);
            terminal::interactive(&mut cmd)?;
        } else {
            let result = hive_setup_support::runner::capture(
                crate::os::spawn,
                "npm",
                &["install".into(), "-g".into(), package(provider).into()],
                INSTALL_TIMEOUT,
            )
            .map_err(io)?;
            if result.code != 0 {
                return Err(invalid(format!(
                    "npm installation failed: {}",
                    result.stderr
                )));
            }
        }
        binary = crate::process::executable(requested);
    }
    Ok(binary)
}

fn pending_login(
    selected: &super::plan::ProviderPlan,
    binary: &Result<std::path::PathBuf>,
    command: &str,
    pending: &mut Vec<String>,
) -> Result<()> {
    if selected.model.is_none() || selected.effort.is_none() {
        return Err(invalid(format!(
            "Without verified login, pass --{}-model and --{}-effort (or configure existing \
                values).",
            selected.provider.as_str(),
            selected.provider.as_str()
        )));
    }
    if binary.is_err() {
        pending.push(format!("npm install -g {}", package(selected.provider)));
    }
    pending.push(format!(
        "{command} auth login {}; {command} setup --provider {} --login skip",
        selected.profile,
        selected.provider.as_str()
    ));
    Ok(())
}

#[derive(serde::Serialize)]
struct LoginConfiguration<'a> {
    auth_profiles: &'a std::collections::BTreeMap<String, crate::config::Profile>,
}

const INSTALL_TIMEOUT: std::time::Duration = Duration::from_secs(600);

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn native_login_configuration_is_private_complete_and_owned_until_return() {
        let (owned, path) = login_configuration(b"auth_profiles: {}\n").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"auth_profiles: {}\n");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(owned.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let directory = owned.path().to_owned();
        drop(owned);
        assert!(!path.exists());
        assert!(!directory.exists());
    }
    #[test]
    fn native_defaults_and_explicit_choices_are_checked_without_guessing() {
        let catalog:Catalog=serde_json::from_value(serde_json::json!({"data":[
            {"model":"first","supportedReasoningEfforts":[{"reasoningEffort":"low"}]},
            {"model":"preferred","isDefault":true,"defaultReasoningEffort":"high","supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"high"}]}
        ]})).unwrap();
        assert_eq!(
            catalog.select(None, None).unwrap(),
            ("preferred".into(), "high".into())
        );
        assert_eq!(
            catalog.select(Some("first"), None).unwrap(),
            ("first".into(), "low".into())
        );
        assert!(catalog.select(Some("invented"), None).is_err());
        assert!(catalog.select(Some("first"), Some("high")).is_err());
    }
}
