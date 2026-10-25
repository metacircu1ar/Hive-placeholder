//! Spawn inputs and ordered validation.
use super::{Deserialize, Input, JsonSchema, Op, Result, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SpawnArgs {
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Configured role selected as a preset; mutually exclusive with role.
    pub preset: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Typed catalog options; optional null clears an inherited spawn setting.
    pub options: Input<crate::options::OptionMap>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Native overlay text; snapshotted, never interpreted as a path.
    pub config: Input<String>,

    /// Up to 10 ordered absolute source paths; private `Hive` state is refused.
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(length(max = crate::runtime::attachments::MAX_FILES))]
    pub attachments: Input<Vec<std::path::PathBuf>>,

    /// Branch from this agent's native conversation after its current turn finishes.
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    pub from_agent: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Optional configured role providing default settings.
    pub role: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Optional display name, 1..64 UTF-8 bytes; not unique and never a target.
    pub name: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    #[schemars(!default, !skip_serializing_if)]
    /// Initial task, 1..65536 UTF-8 bytes.
    pub body: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Absolute workspace directory; ordinary hosted spawns inherit the caller directory.
    pub cwd: Input<std::path::PathBuf>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Login profile or fallback group; branches retain the source login.
    pub auth: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Codex or Claude; defaults from role/config and must match the selected login.
    pub provider: Input<crate::config::Provider>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Native sandbox mode; defaults from role/config.
    pub sandbox: Input<crate::config::Sandbox>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Role instruction override, at most 65536 UTF-8 bytes; empty clears role instructions.
    pub instructions: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Permit configured API fallback credentials, default from role/config.
    pub api_fallback: Input<bool>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Provider model token; defaults from role/config or branch source.
    pub model: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Reasoning effort supported by the chosen model; defaults from role/config or source.
    pub effort: Input<String>,
    #[serde(default, skip_serializing_if = "Input::is_missing")]
    /// Defaults to role: independent survives parent end; `with_parent` expires with the parent.
    pub lifetime: Input<crate::config::Lifetime>,
}

impl SpawnArgs {
    pub(crate) fn options(&self) -> Result<crate::config::SpawnOptions<'_>> {
        self.name()?;
        self.instructions.optional(
            Op::Spawn,
            "/instructions",
            "a string; empty clears role instructions",
        )?;
        let role = selected_role(self)?;
        self.from_agent
            .optional(Op::Spawn, "/from_agent", "a source agent ID string")?;
        if !self.from_agent.is_missing() {
            self.from_agent.required(Op::Spawn, "/from_agent")?;
        }
        let options = crate::config::SpawnOptions {
            role,
            provider: self
                .provider
                .optional(Op::Spawn, "/provider", "codex or claude")?
                .copied(),
            auth: self
                .auth
                .optional(Op::Spawn, "/auth", "a login profile or group string")?
                .map(String::as_str),
            model: self
                .model
                .optional(Op::Spawn, "/model", "a model string")?
                .map(String::as_str),
            effort: self
                .effort
                .optional(Op::Spawn, "/effort", "an effort string")?
                .map(String::as_str),
            options: self
                .options
                .optional(Op::Spawn, "/options", "a catalog option object")?,
            inherited: None,
            inherited_configs: None,
            config: self
                .config
                .optional(Op::Spawn, "/config", "native overlay text")?
                .map(String::as_str),
            sandbox: self
                .sandbox
                .optional(
                    Op::Spawn,
                    "/sandbox",
                    "read-only, workspace-write or full-access",
                )?
                .copied(),
            lifetime: self
                .lifetime
                .optional(Op::Spawn, "/lifetime", "independent or with_parent")?
                .copied(),
            api_fallback: self
                .api_fallback
                .optional(Op::Spawn, "/api_fallback", "a boolean")?
                .copied(),
        };
        super::super::selection::syntax(&options)?;
        if let Some(text) = self.instructions.as_str() {
            hive_input::validate::byte_count(text.len(), crate::limits::CONTROL_INPUT_BYTES)
                .map_err(|rule| {
                    Op::Spawn
                        .error(rule.code(), "/instructions", rule.expected())
                        .with_source(rule)
                })?;
        }
        Ok(options)
    }
}

fn selected_role(args: &SpawnArgs) -> Result<Option<&str>> {
    let role = args
        .role
        .optional(Op::Spawn, "/role", "a configured role string")?;
    let preset = args
        .preset
        .optional(Op::Spawn, "/preset", "a configured role string")?;
    if role.is_some() && preset.is_some() {
        return Err(Op::Spawn.error(
            hive_input::Code::ConflictingArguments,
            "/preset",
            "either role or preset, not both",
        ));
    }
    let role = role.or(preset);
    Ok(role.map(String::as_str))
}
