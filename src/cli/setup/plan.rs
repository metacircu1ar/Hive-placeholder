//! Resolve existing choices before any authentication, initialization or publication.
use super::{
    Result, config, invalid,
    options::{Login, Values},
    publication::Snapshot,
    terminal::Terminal,
};
use crate::config::{Config, Provider};
use serde::Serialize;
use serde_yaml_ng::{Mapping, Value};
use std::collections::BTreeSet;

#[derive(Serialize)]
pub(super) struct ProviderPlan {
    pub provider: Provider,
    pub profile: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub verified: bool,
}
pub(super) struct Plan {
    pub providers: Vec<ProviderPlan>,
    pub default: Provider,
    pub login: Login,
    pub additions: Value,
}
impl Plan {
    pub fn inspect(snapshot: &Snapshot, options: &Values, terminal: &mut Terminal) -> Result<Self> {
        let existing = if snapshot.text.is_empty() {
            None
        } else {
            Some(Config::parse_at(&snapshot.text, snapshot.target())?)
        };
        let providers = providers(options, existing.as_ref(), terminal)?;
        if providers.is_empty()
            || providers.iter().collect::<BTreeSet<_>>().len() != providers.len()
        {
            return Err(choice(
                "--provider",
                "unique codex/claude choices, at least one",
            ));
        }
        let default = default_provider(options, existing.as_ref(), &providers, terminal)?;
        let login = login(options, existing.is_some(), &providers, terminal)?;
        let mut plans = Vec::new();
        let mut profiles = Mapping::new();
        let mut defaults = Mapping::new();
        for provider in providers {
            let (name, model, effort) = provider_values(provider, options, existing.as_ref())?;
            let mut entry = Mapping::new();
            entry.insert("auth".into(), name.clone().into());
            if let Some(model) = &model {
                entry.insert("model".into(), model.clone().into());
            }
            if let Some(effort) = &effort {
                entry.insert("effort".into(), effort.clone().into());
            }
            defaults.insert(provider.as_str().into(), entry.into());
            profiles.insert(
                name.clone().into(),
                serde_yaml_ng::to_value(crate::runtime::api::value(SubscriptionProfile {
                    provider,
                    method: "subscription",
                }))
                .map_err(|cause| invalid("Cannot encode setup profile.").with_source(cause))?,
            );
            plans.push(ProviderPlan {
                provider,
                profile: name,
                model,
                effort,
                verified: false,
            });
        }
        let additions = Value::Mapping(Mapping::from_iter([
            ("auth_profiles".into(), profiles.into()),
            ("default_provider".into(), default.as_str().into()),
            ("providers".into(), defaults.into()),
        ]));
        // Detect explicit conflicts and unsafe text placement before login.
        Config::parse_at(
            &config::patch(&snapshot.text, &additions)?,
            snapshot.target(),
        )?;
        Ok(Self {
            providers: plans,
            default,
            login,
            additions,
        })
    }
    pub fn candidate(&self, snapshot: &Snapshot) -> Result<Config> {
        let value = config::strict_yaml(&config::patch(&snapshot.text, &self.additions)?)?;
        let profiles = Value::Mapping(Mapping::from_iter([(
            "auth_profiles".into(),
            value["auth_profiles"].clone(),
        )]));
        Config::parse_at(&config::yaml(&profiles)?, snapshot.target())
    }
    pub fn complete(&mut self) -> Result<()> {
        for provider in &self.providers {
            let map = self.additions["providers"][provider.provider.as_str()]
                .as_mapping_mut()
                .ok_or_else(|| invalid("Missing provider map."))?;
            map.insert(
                "model".into(),
                provider
                    .model
                    .clone()
                    .ok_or_else(|| invalid("Missing provider model."))?
                    .into(),
            );
            map.insert(
                "effort".into(),
                provider
                    .effort
                    .clone()
                    .ok_or_else(|| invalid("Missing provider effort."))?
                    .into(),
            );
        }
        Ok(())
    }
}
fn parse_provider(text: &str) -> Result<Provider> {
    match text {
        "codex" => Ok(Provider::Codex),
        "claude" => Ok(Provider::Claude),
        _ => Err(choice("--provider", "codex or claude")),
    }
}

fn providers(
    options: &Values,
    existing: Option<&Config>,
    terminal: &mut Terminal,
) -> Result<Vec<Provider>> {
    if let Some(providers) = &options.provider {
        return Ok(providers.clone());
    }
    let existing: BTreeSet<_> = existing
        .map(|c| c.auth_profiles.values().map(|p| p.provider).collect())
        .unwrap_or_default();
    if existing.is_empty() {
        terminal
            .ask(
                "--provider",
                "Providers: codex, claude, or codex,claude",
                Some("codex"),
            )?
            .split(',')
            .map(|s| parse_provider(s.trim()))
            .collect()
    } else {
        Ok(existing.into_iter().collect())
    }
}
fn provider_values(
    provider: Provider,
    options: &Values,
    existing: Option<&Config>,
) -> Result<(String, Option<String>, Option<String>)> {
    let current = existing.as_ref().and_then(|c| c.providers.get(&provider));
    let (name, model, effort) = match provider {
        Provider::Codex => (
            &options.codex_profile,
            &options.codex_model,
            &options.codex_effort,
        ),
        Provider::Claude => (
            &options.claude_profile,
            &options.claude_model,
            &options.claude_effort,
        ),
    };
    let single = existing.as_ref().and_then(|c| {
        let mut selected = c
            .auth_profiles
            .iter()
            .filter(|(_, p)| p.provider == provider);
        let first = selected.next();
        if selected.next().is_none() {
            first.map(|(name, _)| name.clone())
        } else {
            None
        }
    });
    let name = name
        .clone()
        .or_else(|| current.and_then(|p| p.auth.clone()))
        .or(single)
        .unwrap_or_else(|| format!("{}-main", provider.as_str()));
    if !crate::config::valid_name(&name) {
        return Err(invalid("Invalid profile name."));
    }
    let model = model
        .clone()
        .or_else(|| current.and_then(|p| p.model.clone()));
    let effort = effort
        .clone()
        .or_else(|| current.and_then(|p| p.effort.clone()));
    Ok((name, model, effort))
}

fn login(
    options: &Values,
    existing: bool,
    providers: &[Provider],
    terminal: &mut Terminal,
) -> Result<Login> {
    let login = match options.login {
        Some(login) => login,
        None if existing => Login::Skip,
        None => match terminal
            .ask(
                "--login",
                "Login: browser, device, or skip",
                Some("browser"),
            )?
            .as_str()
        {
            "browser" => Login::Browser,
            "device" => Login::Device,
            "skip" => Login::Skip,
            _ => return Err(choice("--login", "browser, device or skip")),
        },
    };
    if login == Login::Device && providers.contains(&Provider::Claude) {
        return Err(choice("--login", "browser or skip when Claude is selected"));
    }
    Ok(login)
}

fn choice(field: &'static str, expected: &str) -> crate::error::Error {
    crate::cli::input::Context::new("setup", field)
        .invalid(
            hive_input::Code::ConflictingArguments,
            "Invalid setup selection.",
            expected,
        )
        .error
}

fn default_provider(
    options: &Values,
    existing: Option<&Config>,
    providers: &[Provider],
    terminal: &mut Terminal,
) -> Result<Provider> {
    let default = options
        .default_provider
        .or(existing.as_ref().and_then(|c| c.default_provider))
        .or_else(|| (providers.len() == 1).then_some(providers[0]));
    let default = match default {
        Some(p) => p,
        None => parse_provider(&terminal.ask(
            "--default-provider",
            "Default provider",
            Some(providers[0].as_str()),
        )?)?,
    };
    if !providers.contains(&default) {
        return Err(choice("--default-provider", "a selected provider"));
    }
    Ok(default)
}

#[derive(Serialize)]
struct SubscriptionProfile {
    provider: Provider,
    method: &'static str,
}
