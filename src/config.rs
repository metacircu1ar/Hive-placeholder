//! Strict configuration parsing and validated runtime limits.
use crate::{
    error::{Error, Result},
    os,
};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    str,
};
use yaml_rust2::scanner::{Scanner, TokenType};
const CONFIG_UNAVAILABLE: &str = "Cannot read the configuration file.";

// Maximum configuration file size accepted for parsing.
pub(crate) const CONFIG_BYTES: usize = 1024 * 1024;
// Read one byte beyond the configuration cap to detect oversized input.
pub(crate) const CONFIG_READ_BYTES: u64 = CONFIG_BYTES as u64 + 1;
// Largest total agent capacity accepted in runtime configuration.
const MAX_CONFIGURED_AGENTS: usize = 1_000_000;
// Largest per-Codex-process session capacity accepted in runtime configuration.
const MAX_SESSIONS_PER_CODEX_PROCESS: usize = 1024;
// Longest configured idle period before an agent releases its resident session.
const MAX_IDLE_SECONDS: u64 = 86400;
// Longest configured minimum residency for a provider cohort.
const MAX_COHORT_SECONDS: u64 = 3600;
// Longest configured idle period for an unused warm provider.
const MAX_WARM_IDLE_SECONDS: u64 = 3600;

#[derive(
    Clone,
    Copy,
    Debug,
    Deserialize,
    Serialize,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    schemars::JsonSchema,
    clap::ValueEnum,
)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Codex,
    Claude,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    Subscription,
    ApiKey,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub provider: Provider,
    pub method: Method,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub api_key_file: Option<PathBuf>,
}

mod auth_groups;
mod auth_profile;
pub use auth_groups::{AuthGroup, AuthMember, AuthMode};
pub(crate) mod deprecations;
pub use auth_profile::KeyFingerprint;
pub(crate) use auth_profile::stored_profile;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(skip)]
    pub(crate) loaded: Option<ConfigurationIdentity>,
    #[serde(skip)]
    pub(crate) deprecations: Vec<deprecations::Notice>,
    pub auth_profiles: BTreeMap<String, Profile>,
    #[serde(default)]
    pub roles: BTreeMap<String, Role>,
    #[serde(default)]
    pub auth_groups: BTreeMap<String, AuthGroup>,
    #[serde(default)]
    pub default_provider: Option<Provider>,
    #[serde(default)]
    pub providers: BTreeMap<Provider, ProviderDefaults>,
    #[serde(default)]
    pub context: Vec<PathBuf>,
    #[serde(default)]
    pub comb: Option<CombBinding>,
    #[serde(default)]
    pub comb_collectors: BTreeMap<String, CollectorConfig>,
    #[serde(default)]
    pub mcp_servers: McpServers,
    #[serde(default)]
    pub runtime: Runtime,
    #[serde(default)]
    pub logging: Logging,
}

/// Identity of the exact configuration snapshot parsed at startup.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationIdentity {
    pub path: PathBuf,
    pub sha256: String,
}

/// Identity capacity and resident execution capacity are independent.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Runtime {
    pub max_agents: usize,
    pub max_loaded: usize,
    pub max_providers: usize,
    pub max_process_variants: usize,
    pub sessions_per_codex_process: usize,
    pub max_starting: usize,
    pub max_active_turns: usize,
    pub idle_seconds: u64,
    pub cohort_min_seconds: u64,
    pub warm_providers: usize,
    pub warm_idle_seconds: u64,
    /// Retain clean, reusable multiplexed processes briefly after their last user.
    pub linger_seconds: u64,
    /// Stream assistant text, tool output and reasoning summaries to watching viewers.
    pub live_stream: bool,
    /// Permit hosted agents to spawn children with a wider sandbox mode.
    pub allow_spawn_widening: bool,
}
// Default total agent capacity when runtime settings are omitted.
const DEFAULT_MAX_AGENTS: usize = 1000;
// Default number of agents that may hold resident sessions simultaneously.
const DEFAULT_MAX_LOADED: usize = 256;
// Default number of provider processes that may coexist.
const DEFAULT_MAX_PROVIDERS: usize = 128;
// Share Codex processes across conversations; Claude remains one per process.
const DEFAULT_SESSIONS_PER_CODEX_PROCESS: usize = 16;
// Default number of agent startups admitted simultaneously.
const DEFAULT_MAX_STARTING: usize = 8;
// Default number of turns allowed to execute simultaneously.
const DEFAULT_MAX_ACTIVE_TURNS: usize = 128;
// Default minimum residency before a provider cohort is eligible for eviction.
const DEFAULT_COHORT_SECONDS: u64 = 5;
// Default idle grace before an unused warm provider exits.
const DEFAULT_WARM_IDLE_SECONDS: u64 = 60;
// Keep an unused multiplexed provider briefly available for its next caller.
const DEFAULT_LINGER_SECONDS: u64 = 2;

impl Default for Runtime {
    fn default() -> Self {
        Self {
            max_agents: DEFAULT_MAX_AGENTS,
            max_loaded: DEFAULT_MAX_LOADED,
            max_providers: DEFAULT_MAX_PROVIDERS,
            max_process_variants: 8,
            sessions_per_codex_process: DEFAULT_SESSIONS_PER_CODEX_PROCESS,
            max_starting: DEFAULT_MAX_STARTING,
            max_active_turns: DEFAULT_MAX_ACTIVE_TURNS,
            idle_seconds: 0,
            cohort_min_seconds: DEFAULT_COHORT_SECONDS,
            warm_providers: 0,
            warm_idle_seconds: DEFAULT_WARM_IDLE_SECONDS,
            linger_seconds: DEFAULT_LINGER_SECONDS,
            live_stream: true,
            allow_spawn_widening: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Role {
    pub provider: Option<Provider>,
    pub auth: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    #[serde(default)]
    pub options: crate::options::OptionMap,
    pub config: Option<PathBuf>,
    pub instructions: Option<PathBuf>,
    #[serde(default)]
    pub comb: Option<CombBinding>,
    #[serde(default)]
    pub mcp_servers: McpServers,
    #[serde(default = "default_api_fallback")]
    pub api_fallback: bool,
    #[serde(default)]
    pub lifetime: Lifetime,
    #[serde(default)]
    pub sandbox: Sandbox,
}
fn default_api_fallback() -> bool {
    true
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDefaults {
    pub auth: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    #[serde(default)]
    pub options: crate::options::OptionMap,
}

mod collectors;
mod logging;
pub use logging::{LogLevel, Logging};
#[cfg(not(feature = "comb"))]
mod features;
pub use collectors::{CollectionSource, CollectorConfig};
mod extensions;
pub use extensions::{CombBinding, McpServer, McpServers};
mod spawning;
pub use spawning::{ResolvedSpawn, SpawnOptions};
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Deserialize,
    Serialize,
    PartialEq,
    Eq,
    schemars::JsonSchema,
    clap::ValueEnum,
)]
#[serde(rename_all = "snake_case")]
#[value(rename_all = "snake_case")]
pub enum Lifetime {
    #[default]
    Independent,
    WithParent,
}
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Deserialize,
    Serialize,
    PartialEq,
    Eq,
    schemars::JsonSchema,
    clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
pub enum Sandbox {
    ReadOnly,
    #[default]
    WorkspaceWrite,
    FullAccess,
}
impl Sandbox {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::FullAccess => "full-access",
        }
    }
    pub fn codex_name(self) -> &'static str {
        match self {
            Self::FullAccess => "danger-full-access",
            _ => self.as_str(),
        }
    }
    pub fn permits_child(self, child: Self) -> bool {
        match self {
            Self::ReadOnly => child == Self::ReadOnly,
            Self::WorkspaceWrite => child != Self::FullAccess,
            Self::FullAccess => true,
        }
    }
}

fn invalid() -> Error {
    Error::config_invalid(
        "Invalid configuration; use unique profile names and supported \
            auth fields.",
    )
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .map_err(|cause| Error::config_unavailable(CONFIG_UNAVAILABLE).with_source(cause))?;
        let file = fs::File::open(&path)
            .map_err(|cause| Error::config_unavailable(CONFIG_UNAVAILABLE).with_source(cause))?;
        let data = os::read_capped(file, CONFIG_READ_BYTES)
            .map_err(|cause| Error::config_unavailable(CONFIG_UNAVAILABLE).with_source(cause))?;
        if data.len() > CONFIG_BYTES {
            return Err(invalid());
        }
        let mut config = Self::parse_at(
            str::from_utf8(&data).map_err(|cause| invalid().with_source(cause))?,
            &path,
        )?;
        config.loaded = Some(ConfigurationIdentity {
            path,
            sha256: format!("{:x}", sha2::Sha256::digest(&data)),
        });
        deprecations::warn(&config.deprecations);
        Ok(config)
    }

    /// Validate text with relative assets resolved beside its configuration file.
    /// The supplied path may be a not-yet-published output target.
    pub fn parse_at(input: &str, path: &Path) -> Result<Self> {
        let path = std::path::absolute(path).map_err(Error::state_io_from)?;
        let directory = path.parent().ok_or_else(invalid)?;
        let mut config = Self::parse_unvalidated(input)?;
        for profile in config.auth_profiles.values_mut() {
            if let Some(file) = &mut profile.api_key_file
                && file.is_relative()
            {
                *file = directory.join(&*file);
            }
        }
        for role in config.roles.values_mut() {
            if let Some(file) = &mut role.config
                && file.is_relative()
            {
                *file = directory.join(&*file);
            }
        }
        config.validate_values()?;
        config.validate_spawning()?;
        Ok(config)
    }

    pub fn parse(input: &str) -> Result<Self> {
        let config = Self::parse_unvalidated(input)?;
        config.validate_values()?;
        config.validate_spawning()?;
        Ok(config)
    }
    fn parse_unvalidated(input: &str) -> Result<Self> {
        let value = Self::yaml(input)?;
        #[cfg(not(feature = "comb"))]
        features::check(&value)?;
        let deprecations = deprecations::inspect(&value, deprecations::TABLE);
        let mut config: Self =
            serde_yaml_ng::from_value(value).map_err(|cause| invalid().with_source(cause))?;
        config.deprecations = deprecations;
        Ok(config)
    }
    pub(crate) fn yaml(input: &str) -> Result<serde_yaml_ng::Value> {
        fn check(value: &serde_yaml_ng::Value) -> bool {
            use serde_yaml_ng::Value;
            match value {
                Value::Mapping(map) => map
                    .iter()
                    .all(|(k, v)| k.as_str().is_some_and(|s| s != "<<") && check(v)),
                Value::Sequence(seq) => seq.iter().all(check),
                Value::Tagged(_) => false,
                _ => true,
            }
        }
        if input.len() > CONFIG_BYTES {
            return Err(invalid());
        }
        // Ban indirection before deserialization so aliases cannot hide configuration.
        let mut scanner = Scanner::new(input.chars());
        if scanner.by_ref().any(|token| {
            matches!(
                token.1,
                TokenType::Anchor(_) | TokenType::Alias(_) | TokenType::Tag(_, _)
            )
        }) {
            return Err(invalid());
        }
        if let Some(cause) = scanner.get_error() {
            return Err(invalid().with_source(cause));
        }
        // Value's mapping deserializer rejects duplicate keys before BTreeMap can replace them.
        let value: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(input).map_err(|cause| invalid().with_source(cause))?;
        if !check(&value) {
            return Err(invalid());
        }
        Ok(value)
    }
    fn validate_values(&self) -> Result<()> {
        self.logging.backend()?;
        let runtime_settings = &self.runtime;
        if runtime_settings.max_agents == 0
            || runtime_settings.max_agents > MAX_CONFIGURED_AGENTS
            || runtime_settings.max_loaded == 0
            || runtime_settings.max_loaded > runtime_settings.max_agents
            || runtime_settings.max_providers == 0
            || runtime_settings.max_providers > runtime_settings.max_loaded
            || runtime_settings.max_process_variants == 0
            || runtime_settings.max_process_variants > MAX_CONFIGURED_AGENTS
            || runtime_settings.sessions_per_codex_process == 0
            || runtime_settings.sessions_per_codex_process > MAX_SESSIONS_PER_CODEX_PROCESS
            || runtime_settings.max_starting == 0
            || runtime_settings.max_starting > runtime_settings.max_loaded
            || runtime_settings.max_active_turns == 0
            || runtime_settings.max_active_turns > runtime_settings.max_loaded
            || runtime_settings.idle_seconds > MAX_IDLE_SECONDS
            || runtime_settings.cohort_min_seconds > MAX_COHORT_SECONDS
            || runtime_settings.warm_providers > runtime_settings.max_providers
            || runtime_settings.warm_idle_seconds == 0
            || runtime_settings.warm_idle_seconds > MAX_WARM_IDLE_SECONDS
            || runtime_settings.linger_seconds > MAX_WARM_IDLE_SECONDS
        {
            return Err(invalid());
        }
        self.auth_profiles.iter().try_for_each(|(name, profile)| {
            if !valid_name(name) {
                return Err(invalid());
            }
            profile.validate()
        })
    }
    pub fn profile(&self, name: &str) -> Result<&Profile> {
        self.auth_profiles.get(name).ok_or(Error::unknown_profile(
            "No configured profile has that name.",
        ))
    }
}

pub(crate) fn valid_name(name: &str) -> bool {
    hive_input::validate::config_name(name).is_ok()
}

pub(crate) fn valid_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

#[cfg(test)]
mod tests;
