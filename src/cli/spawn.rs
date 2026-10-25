//! CLI spawn selection and translation into typed runtime arguments.
use super::input::{Context, Result};
use super::{Input, Request};
use crate::{
    config::{Lifetime, Provider, Sandbox},
    runtime::api::SpawnArgs,
};
use clap::builder::TypedValueParser;
use std::path::PathBuf;

#[derive(clap::Args)]
pub(super) struct Args {
    #[arg(value_parser = hive_input::clap::parsers::config_name)]
    /// Configured role to apply when spawning.
    role: Option<String>,
    /// Apply a configured role as a named settings preset.
    #[arg(long, conflicts_with = "role", value_parser = hive_input::clap::parsers::config_name)]
    preset: Option<String>,
    #[command(flatten)]
    selection: super::agent_options::Options,
    /// Display name (1-64 UTF-8 bytes, no controls or surrounding spaces); not unique.
    #[arg(long, value_parser = hive_input::clap::parsers::agent_name)]
    name: Option<String>,
    /// Existing directory; relative paths resolve against the invoking directory.
    #[arg(long, required_unless_present = "from",
         value_parser = clap::builder::OsStringValueParser::new().map(PathBuf::from))]
    cwd: Option<PathBuf>,
    /// Inherit a saved native conversation at the source's current turn boundary.
    #[arg(long, value_parser = hive_input::clap::parsers::text(crate::limits::CONTROL_INPUT_BYTES))]
    from: Option<String>,
    /// Initial message as literal text.
    #[arg(
        long,
        allow_hyphen_values = true,
        required_unless_present = "body_file",
        conflicts_with = "body_file"
    )]
    body: Option<String>,
    #[arg(long, required_unless_present = "body", conflicts_with = "body")]
    /// UTF-8 initial/message text file (at most 65536 bytes).
    body_file: Option<PathBuf>,
    /// Attach a file snapshot (repeatable, at most 10); oversized files stay live paths.
    #[arg(long = "attach", value_name = "PATH")]
    attachments: Vec<PathBuf>,
    #[arg(long, value_enum)]
    /// Native provider to use: codex or claude.
    provider: Option<Provider>,
    #[arg(long, value_parser = hive_input::clap::parsers::config_name)]
    /// Configured login profile or fallback group.
    auth: Option<String>,
    #[arg(long, value_parser = hive_input::clap::parsers::model)]
    /// Provider model token (1-128 ASCII bytes); validated against provider discovery.
    model: Option<String>,
    #[arg(long, value_parser = hive_input::clap::parsers::effort)]
    /// Reasoning effort supported by the selected model.
    effort: Option<String>,
    #[arg(long)]
    /// Additional UTF-8 instructions file (at most 65536 bytes; empty allowed).
    instructions_file: Option<PathBuf>,
    #[arg(long, value_enum)]
    /// Native sandbox policy: read-only, workspace-write or full-access.
    sandbox: Option<Sandbox>,
    /// Human spawns have no parent; only independent can be selected here.
    #[arg(long, value_parser = human_lifetime())]
    lifetime: Option<Lifetime>,
    #[arg(long)]
    /// Disable API-key fallback for this agent.
    no_api_fallback: bool,
    #[arg(long, value_parser = hive_input::clap::parsers::request_id)]
    /// Stable retry key (1-128 bytes, no controls); reuse with identical input.
    pub request_id: Option<String>,
}

impl Args {
    pub(super) fn validate_options(&self) -> Result<()> {
        self.selection.values("spawn").map(|_| ())
    }

    pub(super) fn run(
        &self,
        root: &std::path::Path,
        id: &str,
        request: Request,
    ) -> crate::error::Result<super::CommandOutcome> {
        let mut outcome = super::remote_response(root, id, request)?;
        if let Some(input) = outcome.input.take() {
            let input = input
                .file_source("spawn", "--body", "--body-file", self.body_file.as_deref())?
                .file_source("spawn", "--cwd", "--cwd", self.cwd.as_deref())?
                .file_source(
                    "spawn",
                    "--instructions-file",
                    "--instructions-file",
                    self.instructions_file.as_deref(),
                )?;
            outcome.value = input.value();
            outcome.input = Some(input);
        }
        Ok(outcome)
    }

    pub(super) fn request(&self) -> Result<Request> {
        let (options, config) = self.selection.read("spawn")?;
        let cwd = self
            .cwd
            .as_deref()
            .map(|path| Context::new("spawn", "--cwd").file(path).directory())
            .transpose()?;
        Ok(Request::Spawn(Box::new(SpawnArgs {
            preset: self.preset.clone().map_or(Input::Missing, Input::Valid),
            options,
            config,
            attachments: super::input::attachment_paths("spawn", &self.attachments)?,
            role: self.role.clone().map_or(Input::Missing, Input::from),
            name: self.name.clone().map_or(Input::Missing, Input::from),
            cwd: cwd.map_or(Input::Missing, Input::from),
            from_agent: self.from.clone().map_or(Input::Missing, Input::from),
            body: super::input::text_source(
                "spawn",
                "--body",
                "--body-file",
                self.body.as_deref(),
                self.body_file.as_deref(),
            )?
            .into(),
            provider: self.provider.map_or(Input::Missing, Input::from),
            auth: self.auth.clone().map_or(Input::Missing, Input::from),
            model: self.model.clone().map_or(Input::Missing, Input::from),
            effort: self.effort.clone().map_or(Input::Missing, Input::from),
            lifetime: self.lifetime.map_or(Input::Missing, Input::from),
            sandbox: self.sandbox.map_or(Input::Missing, Input::from),
            instructions: self
                .instructions_file
                .as_deref()
                .map(instructions)
                .transpose()?
                .map_or(Input::Missing, Input::from),
            api_fallback: if self.no_api_fallback {
                false.into()
            } else {
                Input::Missing
            },
        })))
    }
}

fn instructions(path: &std::path::Path) -> Result<String> {
    Context::new("spawn", "--instructions-file")
        .file(path)
        .text(super::BODY_BYTES, true)
}

fn human_lifetime() -> impl clap::builder::TypedValueParser<Value = Lifetime> {
    clap::builder::PossibleValuesParser::new(["independent"]).map(|_| Lifetime::Independent)
}
