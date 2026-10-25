//! Explicit CLI values override the same keys in a strict answers document.
use super::{Result, invalid};
use crate::config::Provider;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
mod answers;
pub(super) mod removal;
mod supplied;
mod validation;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Login {
    Browser,
    Device,
    Skip,
}

#[derive(clap::Args)]
pub(crate) struct Options {
    /// Strict YAML answers; flags override the same named keys.
    #[arg(long)]
    answers: Option<PathBuf>,
    #[command(flatten)]
    values: Values,
}
#[derive(Clone, Default, clap::Args, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub(super) struct Values {
    /// Remove the installed `Hive` commands and owned services; retain user data.
    #[arg(long, num_args=0..=1, default_missing_value="true")]
    pub uninstall: Option<bool>,
    /// With --uninstall, purge owned data while retaining native provider history.
    #[arg(long, num_args=0..=1, default_missing_value="true")]
    pub purge: Option<bool>,
    /// Explicit selected Comb root to remove, including all knowledge pages.
    #[arg(long)]
    pub purge_comb_root: Option<PathBuf>,
    /// Installation prefix for removal; normally inferred from the invoked command.
    #[arg(long)]
    pub prefix: Option<PathBuf>,
    /// Select one or both providers, separated by commas.
    #[arg(long, value_delimiter = ',')]
    pub provider: Option<Vec<Provider>>,
    #[arg(long)]
    /// Default provider used when a role does not select one.
    pub default_provider: Option<Provider>,
    /// Native login method. Device login is Codex-only.
    #[arg(long)]
    pub login: Option<Login>,
    #[arg(long)]
    /// Name of the Codex subscription profile to configure.
    pub codex_profile: Option<String>,
    #[arg(long)]
    /// Name of the Claude subscription profile to configure.
    pub claude_profile: Option<String>,
    #[arg(long, value_parser = hive_input::clap::parsers::model)]
    /// Default Codex model token; discovered after login if omitted.
    pub codex_model: Option<String>,
    #[arg(long, value_parser = hive_input::clap::parsers::model)]
    /// Default Claude model token; discovered after login if omitted.
    pub claude_model: Option<String>,
    #[arg(long, value_parser = hive_input::clap::parsers::effort)]
    /// Default Codex reasoning effort, validated against its model.
    pub codex_effort: Option<String>,
    #[arg(long, value_parser = hive_input::clap::parsers::effort)]
    /// Default Claude reasoning effort, validated against its model.
    pub claude_effort: Option<String>,
    #[arg(long)]
    /// Knowledge root for setup, or its selected integration scope with --uninstall --purge.
    pub comb_root: Option<PathBuf>,
    #[arg(long)]
    /// HTTP embedding configuration for semantic search; omit for text search only.
    pub embedding_config: Option<PathBuf>,
    #[arg(long)]
    /// Hive-to-Comb connection file; removal selection requires --uninstall --purge.
    pub bridge_file: Option<PathBuf>,
    #[arg(long)]
    /// Installed Comb executable to invoke during setup.
    pub comb_bin: Option<PathBuf>,
    #[arg(long, num_args=0..=1, default_missing_value="true")]
    /// Skip knowledge setup and its role bindings.
    pub no_comb: Option<bool>,
    #[arg(long, num_args=0..=1, default_missing_value="true")]
    /// Do not bind newly configured roles to Comb.
    pub no_comb_roles: Option<bool>,
    #[arg(long, num_args=0..=1, default_missing_value="true")]
    /// Configure automatic evidence delivery to Comb.
    pub collectors: Option<bool>,
    /// Copy the bundled guide and its linked documentation into user skill directories.
    #[arg(long, value_delimiter = ',')]
    pub user_skills: Option<Vec<Provider>>,
    #[arg(long)]
    /// Bundled skill directory to use for provisioning.
    pub skills_dir: Option<PathBuf>,
    /// Explicitly permit npm installation of a missing provider CLI.
    #[arg(long, value_delimiter = ',')]
    pub install_provider: Option<Vec<Provider>>,
    #[arg(long, num_args=0..=1, default_missing_value="true")]
    /// Install and start owned user login services (default true).
    pub services: Option<bool>,
    #[arg(long, num_args=0..=1, default_missing_value="true")]
    /// Leave services unmanaged; opposite of --services.
    pub no_services: Option<bool>,
    /// Permit interrupting the owned running daemon to load changed configuration.
    #[arg(long, num_args=0..=1, default_missing_value="true")]
    pub restart_daemon: Option<bool>,
    /// Installer coordination: existing stopped scopes remain stopped.
    #[arg(long, hide = true, num_args=0..=1, default_missing_value="true")]
    pub preserve_stopped_services: Option<bool>,
    #[arg(long)]
    /// Private environment file for managed service processes.
    pub service_environment: Option<PathBuf>,
    #[arg(long)]
    /// Executable search PATH for managed services.
    pub service_path: Option<String>,
    #[arg(long, num_args=0..=1, default_missing_value="true")]
    /// Apply supplied answers without interactive prompts; confirm removal when uninstalling.
    pub yes: Option<bool>,
    #[arg(long, num_args=0..=1, default_missing_value="true")]
    /// Preview setup or removal without applying changes.
    pub dry_run: Option<bool>,
}
pub(super) struct Resolved {
    pub values: Values,
    answer_fields: std::collections::BTreeSet<String>,
}
impl Options {
    pub(super) fn resolve(&self) -> Result<Resolved> {
        if self
            .values
            .services
            .zip(self.values.no_services)
            .is_some_and(|(yes, no)| yes == no)
        {
            return Err(removal::invalid(
                "--services",
                "Conflicting service switches.",
                "choose --services or --no-services, or supply opposite boolean values",
            ));
        }
        let mut resolved = self.merge()?;
        let result = &mut resolved.values;
        if result.uninstall == Some(true) {
            result
                .validate()
                .map_err(|error| resolved.attribute(error))?;
            return Ok(resolved);
        }
        // Explicit opposite switches supersede an answers-file preference.
        if let Some(no) = self.values.no_services {
            result.services = Some(!no);
            resolved.answer_fields.remove("services");
        }
        if let Some(yes) = self.values.services {
            result.no_services = Some(!yes);
            resolved.answer_fields.remove("no-services");
        }
        if result.services.is_none() {
            result.services = result.no_services.map(|v| !v);
        }
        if result.no_services.is_none() {
            result.no_services = result.services.map(|v| !v);
        }
        let checked = result.validate().and_then(|()| result.validate_supplied());
        checked.map_err(|error| resolved.attribute(error))?;
        Ok(resolved)
    }

    fn merge(&self) -> Result<Resolved> {
        let mut value = match &self.answers {
            Some(path) => answers::read(path)?,
            None => answers::encoded(&Values::default())?,
        };
        let mut answer_fields: std::collections::BTreeSet<String> = value
            .as_object()
            .ok_or_else(|| invalid("Invalid setup answers shape."))?
            .iter()
            .filter(|(_, value)| !value.is_null())
            .map(|(key, _)| key.clone())
            .collect();
        let flags = answers::encoded(&self.values)?;
        for (key, field) in flags
            .as_object()
            .ok_or_else(|| invalid("Invalid setup flags."))?
        {
            if !field.is_null() {
                value[key] = field.clone();
                answer_fields.remove(key);
            }
        }
        let values = serde_json::from_value(value).map_err(|cause| {
            crate::error::Error::internal_error("Cannot merge setup options.").with_source(cause)
        })?;
        Ok(Resolved {
            values,
            answer_fields,
        })
    }
}
impl Values {
    pub(super) fn positive_comb(&self) -> bool {
        self.comb_root.is_some()
            || self.embedding_config.is_some()
            || self.bridge_file.is_some()
            || self.collectors == Some(true)
            || self.no_comb_roles == Some(false)
    }
}
