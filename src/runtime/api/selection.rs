//! Request selections keep caller overrides distinct from config and inherited state.
use super::{InputContext as Op, SetAuthArgs, SetOptionsArgs, SpawnArgs};
use crate::{
    config::{Config, Provider, ResolvedSpawn, Role, SpawnOptions},
    error::{Error, Result},
    runtime::{auth_fallback::CatalogMiss, domain::Agent, provider::protocol::Model},
};
use hive_input::{Code, validate};

pub(super) fn syntax(options: &SpawnOptions<'_>) -> Result<()> {
    for (value, field) in [(options.role, "/role"), (options.auth, "/auth")] {
        if let Some(value) = value {
            rule(Op::Spawn, field, validate::config_name(value))?;
        }
    }
    if let Some(model) = options.model {
        rule(Op::Spawn, "/model", validate::model(model))?;
    }
    if let Some(effort) = options.effort {
        rule(Op::Spawn, "/effort", validate::effort(effort))?;
    }
    Ok(())
}

fn rule(op: Op, field: &str, result: std::result::Result<(), validate::Violation>) -> Result<()> {
    result.map_err(|cause| {
        op.error(cause.code(), field, cause.expected())
            .with_source(cause)
    })
}

pub(in crate::runtime) fn role<'a>(
    config: &'a Config,
    name: Option<&str>,
) -> Result<Option<&'a Role>> {
    name.map(|name| {
        config
            .roles
            .get(name)
            .ok_or_else(|| Op::Spawn.error(Code::InvalidValue, "/role", "a configured role name"))
    })
    .transpose()
}

fn auth_provider(config: &Config, name: &str, op: Op) -> Result<Provider> {
    rule(op, "/auth", validate::config_name(name))?;
    if !config.auth_profiles.contains_key(name) && !config.auth_groups.contains_key(name) {
        return Err(op.error(
            Code::InvalidValue,
            "/auth",
            "a configured login profile or group",
        ));
    }
    // A broken configured group is not an unknown caller-supplied name.
    config.auth_provider(name)
}

/// A display name: a label, not an identifier.
pub(in crate::runtime) fn name(op: Op, value: &str) -> Result<()> {
    rule(op, "/name", validate::agent_name(value))
}

impl super::SetNameArgs {
    /// The requested name, or none to clear it: exactly one of `name` and
    /// `clear: true`.
    pub(in crate::runtime) fn name(&self) -> Result<Option<&str>> {
        let name = self
            .name
            .optional(Op::SetName, "/name", "a name string")?
            .map(String::as_str);
        let clear = self
            .clear
            .optional(Op::SetName, "/clear", "a boolean")?
            .copied()
            .unwrap_or(false);
        match (name, clear) {
            (Some(_), true) => Err(Op::SetName.error(
                Code::ConflictingArguments,
                "/clear",
                "either name or clear, not both",
            )),
            (None, false) => Err(Op::SetName.error(
                Code::MissingArgument,
                "/name",
                "a name string, or clear: true",
            )),
            (name, _) => {
                if let Some(name) = name {
                    self::name(Op::SetName, name)?;
                }
                Ok(name)
            }
        }
    }
}

impl SpawnArgs {
    /// The new agent's display name, when the request gives one.
    pub(in crate::runtime) fn name(&self) -> Result<Option<&str>> {
        let name = self
            .name
            .optional(Op::Spawn, "/name", "a name string")?
            .map(String::as_str);
        if let Some(name) = name {
            self::name(Op::Spawn, name)?;
        }
        Ok(name)
    }

    pub(in crate::runtime) fn resolve(&self, config: &Config) -> Result<ResolvedSpawn> {
        self.resolve_parent(config, None)
    }

    pub(in crate::runtime) fn resolve_parent(
        &self,
        config: &Config,
        parent: Option<&Agent>,
    ) -> Result<ResolvedSpawn> {
        let mut options = self.options()?;
        let role = role(config, options.role)?;
        let explicit_auth = options
            .auth
            .map(|name| auth_provider(config, name, Op::Spawn))
            .transpose()?;
        if let Some(provider) = options.provider {
            let auth = explicit_auth.or(role
                .and_then(|role| role.auth.as_deref())
                .map(|name| config.auth_provider(name))
                .transpose()?);
            if auth.is_some_and(|auth| auth != provider) {
                return Err(Op::Spawn.error(
                    Code::ConflictingArguments,
                    "/provider",
                    "the provider of the selected login or role login default",
                ));
            }
        }
        let provider = config.spawn_provider_for(&options)?;
        let parent = parent.filter(|parent| parent.profile.provider == provider);
        let inherited = parent.map(crate::runtime::branching::effective_selection);
        let inherited_configs =
            parent.map(|parent| parent.agent_options.inherited_configs(&parent.id));
        if let Some((parent, selection)) = parent.zip(inherited.as_ref()) {
            options.inherited = Some((&selection.options, &parent.id));
        }
        options.inherited_configs = inherited_configs.as_deref();
        config.resolve_spawn(options).map_err(spawn_options_error)
    }

    pub(in crate::runtime) fn catalog_error(&self, error: Error) -> Error {
        use std::error::Error as _;
        let Some(miss) = error
            .source()
            .and_then(|cause| cause.downcast_ref::<CatalogMiss>())
        else {
            return error;
        };
        let has_model = !self.model.is_missing()
            || self
                .options
                .get()
                .is_some_and(|values| values.contains_key("model"));
        let has_effort = !self.effort.is_missing()
            || self
                .options
                .get()
                .is_some_and(|values| values.contains_key("effort"));
        let model_field = if self.model.is_missing() {
            "/options/model"
        } else {
            "/model"
        };
        let effort_field = if self.effort.is_missing() {
            "/options/effort"
        } else {
            "/effort"
        };
        let (field, expected) = match miss {
            CatalogMiss::Model if has_model => {
                (model_field, "a model in a selected login's catalog")
            }
            CatalogMiss::Effort if has_effort => {
                (effort_field, "an effort supported by the selected model")
            }
            CatalogMiss::Effort if has_model => (
                model_field,
                "a model supporting the inherited effort, or an explicit supported effort",
            ),
            _ => return error,
        };
        Op::Spawn
            .error(Code::InvalidValue, field, expected)
            .with_source(error)
    }
}

impl SetAuthArgs {
    pub(in crate::runtime) fn selection<'a>(
        &'a self,
        config: &Config,
        provider: Provider,
    ) -> Result<&'a str> {
        let auth = self.auth.required(Op::SetAuth, "/auth")?;
        if auth_provider(config, auth, Op::SetAuth)? != provider {
            return Err(Op::SetAuth.error(
                Code::ConflictingArguments,
                "/auth",
                "a login profile or group belonging to the agent's existing provider",
            ));
        }
        Ok(auth)
    }
}

impl SetOptionsArgs {
    pub(in crate::runtime) fn selection(
        &self,
        agent: &Agent,
    ) -> Result<crate::options::AgentOptions> {
        let changes = self.options.optional(
            Op::SetOptions,
            "/options",
            "a partial catalog option object",
        )?;
        let config = self
            .config
            .optional(Op::SetOptions, "/config", "native overlay text")?;
        if changes.is_none() && config.is_none() {
            return Err(Op::SetOptions.error(
                Code::MissingArgument,
                "/options",
                "a partial catalog option object or native configuration",
            ));
        }
        let empty = crate::options::OptionMap::new();
        let changes = changes.unwrap_or(&empty);
        crate::options::catalog(agent.profile.provider)?.validate(changes, true)?;
        let config = config
            .map(|text| {
                crate::options::NativeConfig::parse(agent.profile.provider, text, "set_options")
            })
            .transpose()?;
        if let Some(Some(config)) = &config
            && changes.keys().any(|key| config.values.contains_key(key))
        {
            return Err(Op::SetOptions.error(
                Code::ConflictingArguments,
                "/config",
                "do not repeat an option in both options and native configuration",
            ));
        }
        let selected = agent
            .agent_options
            .changed(agent.profile.provider, changes, config)?;
        Ok(selected)
    }

    pub(in crate::runtime) fn check_catalog(
        &self,
        snapshot: &crate::options::Snapshot,
        provider: Provider,
        catalog: &[Model],
    ) -> Result<()> {
        let model = snapshot
            .string("model")
            .ok_or_else(|| Error::database_error("Agent options have no model."))?;
        let effort = snapshot
            .string("effort")
            .ok_or_else(|| Error::database_error("Agent options have no effort."))?;
        let selected = catalog
            .iter()
            .find(|entry| entry.name() == Some(model))
            .ok_or_else(|| {
                Op::SetOptions.error(
                    Code::InvalidValue,
                    "/options/model",
                    "a model in the provider catalog",
                )
            })?;
        if !selected.supports_effort(effort) {
            return Err(Op::SetOptions.error(
                Code::InvalidValue,
                if self
                    .options
                    .get()
                    .is_some_and(|values| values.contains_key("effort"))
                {
                    "/options/effort"
                } else {
                    "/options/model"
                },
                "a model and effort supported together; supply an explicit supported effort",
            ));
        }
        crate::options::catalog(provider)?
            .validate_capabilities(&snapshot.values, selected.resolved_name().unwrap_or(model))
    }
}

pub(in crate::runtime) fn spawn_options_error(mut error: Error) -> Error {
    if let Error::Input { input, .. } = &mut error {
        let replaced = hive_input::InputError::new(
            input.code(),
            input.message(),
            input.field(),
            input.expected(),
            vec![Op::Spawn.example().into()],
        );
        match replaced {
            Ok(value) => **input = value,
            Err(cause) => {
                return Error::internal_error("Invalid spawn option error metadata.")
                    .with_source(cause);
            }
        }
    }
    error
}
