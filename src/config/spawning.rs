//! Role defaults and validated agent spawn configuration.
use super::{Config, Error, Lifetime, Provider, Result, Sandbox, valid_name};
use std::{collections::BTreeSet, fs, path::PathBuf};

#[derive(Default)]
pub struct SpawnOptions<'a> {
    pub role: Option<&'a str>,
    pub provider: Option<Provider>,
    pub auth: Option<&'a str>,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub options: Option<&'a crate::options::OptionMap>,
    pub inherited: Option<(&'a crate::options::Snapshot, &'a str)>,
    pub inherited_configs: Option<&'a [crate::options::NativeConfig]>,
    pub config: Option<&'a str>,
    pub lifetime: Option<Lifetime>,
    pub sandbox: Option<Sandbox>,
    pub api_fallback: Option<bool>,
}

#[derive(Clone)]
pub struct ResolvedSpawn {
    pub provider: Provider,
    pub auth: String,
    pub model: String,
    pub effort: String,
    pub options: crate::options::Snapshot,
    pub native_configs: Vec<crate::options::NativeConfig>,
    pub instructions: Option<PathBuf>,
    pub comb: Option<super::CombBinding>,
    pub mcp_servers: super::McpServers,
    pub lifetime: Lifetime,
    pub sandbox: Sandbox,
    pub api_fallback: bool,
}

fn missing(key: &str) -> Error {
    Error::config_invalid(match key {
        "default_provider" => "Missing spawn default: default_provider.",
        "providers.codex.auth" => "Missing spawn default: providers.codex.auth.",
        "providers.claude.auth" => "Missing spawn default: providers.claude.auth.",
        "providers.codex.model" => "Missing spawn default: providers.codex.model.",
        _ => "Missing spawn default: providers.claude.model.",
    })
}

impl Config {
    pub fn auth_provider(&self, auth: &str) -> Result<Provider> {
        let member = self
            .auth_groups
            .get(auth)
            .and_then(|group| group.members.first());
        self.profile(member.map_or(auth, super::AuthMember::profile))
            .map(|p| p.provider)
    }

    pub fn auth_members(&self, auth: &str) -> Result<Vec<String>> {
        if let Some(group) = self.auth_groups.get(auth) {
            return Ok(group.profiles().map(str::to_owned).collect());
        }
        self.profile(auth)?;
        Ok(vec![auth.to_owned()])
    }

    pub fn resolve_spawn(&self, options: SpawnOptions<'_>) -> Result<ResolvedSpawn> {
        let role = options
            .role
            .map(|name| {
                self.roles
                    .get(name)
                    .ok_or_else(|| Error::unknown_role("No configured role has that name."))
            })
            .transpose()?;
        let role_auth = role.and_then(|r| r.auth.as_deref());
        let provider = self.spawn_provider(&options, role, role_auth)?;
        let defaults = self.providers.get(&provider);
        let profiles: Vec<_> = self
            .auth_profiles
            .iter()
            .filter(|(_, p)| p.provider == provider)
            .collect();
        let auth = options
            .auth
            .or(role_auth)
            .or(defaults.and_then(|d| d.auth.as_deref()))
            .or_else(|| (profiles.len() == 1).then(|| profiles[0].0.as_str()))
            .ok_or_else(|| missing(&format!("providers.{}.auth", provider.as_str())))?;
        if self.auth_provider(auth)? != provider {
            return Err(Error::config_invalid(
                "Resolved provider and auth contradict each other.",
            ));
        }
        let (settings, native_configs) = self.spawn_settings(&options, role, provider)?;
        let model = settings
            .string("model")
            .ok_or_else(|| missing(&format!("providers.{}.model", provider.as_str())))?;
        let effort = settings
            .string("effort")
            .ok_or_else(|| Error::config_invalid("Missing resolved reasoning effort."))?;
        validate_model_effort(Some(model), Some(effort))?;
        let comb = role
            .and_then(|r| r.comb.clone())
            .or_else(|| self.comb.clone());
        let mut mcp_servers = self.mcp_servers.clone();
        if let Some(role) = role {
            mcp_servers.extend(role.mcp_servers.clone());
        }
        if !cfg!(feature = "comb") && comb.is_some() {
            return Err(Error::config_invalid(
                "Comb bindings require an Comb-capable Hive build.",
            ));
        }
        if let Some(binding) = &comb {
            binding.validate()?;
        }
        super::extensions::validate_servers(&mcp_servers)?;
        Ok(ResolvedSpawn {
            comb,
            mcp_servers,
            provider,
            auth: auth.into(),
            model: model.into(),
            effort: effort.into(),
            options: settings,
            native_configs,
            instructions: role.and_then(|r| r.instructions.clone()),
            lifetime: options
                .lifetime
                .or(role.map(|r| r.lifetime))
                .unwrap_or_default(),
            sandbox: options
                .sandbox
                .or(role.map(|r| r.sandbox))
                .unwrap_or_default(),
            api_fallback: options
                .api_fallback
                .or(role.map(|r| r.api_fallback))
                .unwrap_or(true),
        })
    }

    fn spawn_settings(
        &self,
        options: &SpawnOptions<'_>,
        role: Option<&super::Role>,
        provider: Provider,
    ) -> Result<(crate::options::Snapshot, Vec<crate::options::NativeConfig>)> {
        let defaults = self.providers.get(&provider);
        let empty = crate::options::OptionMap::new();
        let role_source = format!("role:{}", options.role.unwrap_or_default());
        let role_overlay = role
            .and_then(|role| role.config.as_deref())
            .map(|path| read_overlay(path, provider, &role_source))
            .transpose()?
            .flatten();
        let spawn_overlay = options
            .config
            .map(|text| crate::options::NativeConfig::parse(provider, text, "spawn"))
            .transpose()?
            .flatten();
        let settings = crate::options::resolve(
            provider,
            defaults.map(|defaults| crate::options::Layer {
                source: "provider_default",
                options: &defaults.options,
                model: defaults.model.as_deref(),
                effort: defaults.effort.as_deref(),
                overlay: None,
            }),
            options.inherited,
            role.map(|role| crate::options::Layer {
                source: &role_source,
                options: &role.options,
                model: role.model.as_deref(),
                effort: role.effort.as_deref(),
                overlay: role_overlay.as_ref(),
            }),
            crate::options::Layer {
                source: "spawn",
                options: options.options.unwrap_or(&empty),
                model: options.model,
                effort: options.effort,
                overlay: spawn_overlay.as_ref(),
            },
        )?;
        let mut native_configs = options.inherited_configs.unwrap_or_default().to_vec();
        native_configs.extend(role_overlay);
        native_configs.extend(spawn_overlay);
        Ok((settings, native_configs))
    }

    pub(crate) fn spawn_provider_for(&self, options: &SpawnOptions<'_>) -> Result<Provider> {
        let role = options
            .role
            .map(|name| {
                self.roles
                    .get(name)
                    .ok_or_else(|| Error::unknown_role("No configured role has that name."))
            })
            .transpose()?;
        self.spawn_provider(options, role, role.and_then(|role| role.auth.as_deref()))
    }

    fn spawn_provider(
        &self,
        options: &SpawnOptions<'_>,
        role: Option<&super::Role>,
        role_auth: Option<&str>,
    ) -> Result<Provider> {
        let explicit_auth_provider = options
            .auth
            .map(|auth| self.auth_provider(auth))
            .transpose()?;
        if options
            .provider
            .zip(explicit_auth_provider)
            .is_some_and(|(a, b)| a != b)
        {
            return Err(Error::config_invalid(
                "Explicit provider and auth contradict each other.",
            ));
        }
        let implicit: BTreeSet<_> = self.auth_profiles.values().map(|p| p.provider).collect();
        explicit_auth_provider
            .or(options.provider)
            .or(role.and_then(|r| r.provider))
            .or(role_auth.map(|auth| self.auth_provider(auth)).transpose()?)
            .or(self.default_provider)
            .or_else(|| implicit.first().copied().filter(|_| implicit.len() == 1))
            .ok_or_else(|| missing("default_provider"))
    }

    pub(super) fn validate_spawning(&self) -> Result<()> {
        crate::options::catalog(Provider::Codex)?;
        crate::options::catalog(Provider::Claude)?;
        super::collectors::validate(&self.comb_collectors)?;
        if let Some(comb) = &self.comb {
            comb.validate()?;
        }
        super::extensions::validate_servers(&self.mcp_servers)?;
        self.auth_groups
            .iter()
            .try_for_each(|(name, members)| self.validate_auth_group(name, members))?;
        self.providers.iter().try_for_each(|(provider, defaults)| {
            crate::options::resolve(
                *provider,
                Some(crate::options::Layer {
                    source: "provider_default",
                    options: &defaults.options,
                    model: defaults.model.as_deref(),
                    effort: defaults.effort.as_deref(),
                    overlay: None,
                }),
                None,
                None,
                crate::options::Layer {
                    source: "spawn",
                    options: &crate::options::OptionMap::new(),
                    model: None,
                    effort: None,
                    overlay: None,
                },
            )
            .map_err(|cause| {
                Error::config_invalid("Invalid provider options.").with_source(cause)
            })?;
            if let Some(auth) = &defaults.auth
                && self.auth_provider(auth)? != *provider
            {
                return Err(Error::config_invalid(
                    "Provider default auth belongs to another provider.",
                ));
            }
            validate_model_effort(defaults.model.as_deref(), defaults.effort.as_deref())
        })?;
        self.roles.iter().try_for_each(|(name, role)| {
            if !valid_name(name) {
                return Err(Error::config_invalid("Invalid role name."));
            }
            if let (Some(provider), Some(auth)) = (role.provider, &role.auth)
                && self.auth_provider(auth)? != provider
            {
                return Err(Error::config_invalid(
                    "Role provider and auth contradict each other.",
                ));
            }
            self.resolve_spawn(SpawnOptions {
                role: Some(name),
                ..SpawnOptions::default()
            })
            .map_err(|cause| Error::config_invalid("Invalid role options.").with_source(cause))?;
            Ok(())
        })
    }
}

fn read_overlay(
    path: &std::path::Path,
    provider: Provider,
    source: &str,
) -> Result<Option<crate::options::NativeConfig>> {
    let file = fs::File::open(path).map_err(|cause| {
        Error::config_invalid("Cannot read role native configuration.").with_source(cause)
    })?;
    let bytes = crate::os::read_capped(file, crate::options::NATIVE_CONFIG_BYTES as u64 + 1)
        .map_err(|cause| {
            Error::config_invalid("Cannot read role native configuration.").with_source(cause)
        })?;
    if bytes.len() > crate::options::NATIVE_CONFIG_BYTES {
        return Err(Error::config_invalid(
            "Role native configuration exceeds 64 KiB.",
        ));
    }
    let text = std::str::from_utf8(&bytes).map_err(|cause| {
        Error::config_invalid("Role native configuration must be UTF-8.").with_source(cause)
    })?;
    crate::options::NativeConfig::parse(provider, text, source).map_err(|cause| {
        Error::config_invalid("Invalid role native configuration.").with_source(cause)
    })
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }
}

fn validate_model_effort(model: Option<&str>, effort: Option<&str>) -> Result<()> {
    if model.is_some_and(|value| hive_input::validate::model(value).is_err())
        || effort.is_some_and(|value| hive_input::validate::effort(value).is_err())
    {
        return Err(Error::config_invalid("Invalid model or reasoning effort."));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
