//! Once-read catalog values and native overlay text for spawn and set-options.
use super::{
    Input, PathBuf,
    input::{Context, Result},
};
use crate::options::{MAX_OPTIONS, NATIVE_CONFIG_BYTES, OptionMap};
use hive_input::Code;
use serde_json::Value;

#[derive(clap::Args, Default)]
pub(super) struct Options {
    /// Catalog KEY=VALUE, repeatable. JSON scalars are typed; other values are strings.
    #[arg(long = "option", value_name = "KEY=VALUE")]
    option: Vec<String>,
    /// Native Codex TOML or Claude settings JSON file, read once (at most 64 KiB).
    #[arg(long)]
    native_config: Option<PathBuf>,
}
impl Options {
    pub(super) fn values(&self, command: &'static str) -> Result<OptionMap> {
        let context = Context::new(command, "--option");
        let mut values = OptionMap::new();
        for raw in &self.option {
            let (key, value) = raw
                .split_once('=')
                .filter(|(key, _)| !key.is_empty())
                .ok_or_else(|| {
                    context.invalid(
                        Code::InvalidValue,
                        "Invalid option assignment.",
                        "KEY=VALUE from hivectl options catalog",
                    )
                })?;
            let value =
                serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_owned()));
            if values.insert(key.to_owned(), value).is_some() || values.len() > MAX_OPTIONS {
                return Err(context.invalid(
                    Code::ConflictingArguments,
                    "Duplicate or excessive option assignments.",
                    "at most 64 unique catalog keys",
                ));
            }
        }
        crate::options::validate_unbound(&values, command == "spawn").map_err(|error| {
            if let crate::error::Error::Input { input, .. } = &error {
                context
                    .invalid(input.code(), "Invalid agent options.", input.expected())
                    .with_source(error)
            } else {
                error.into()
            }
        })?;
        Ok(values)
    }

    pub(super) fn read(&self, command: &'static str) -> Result<(Input<OptionMap>, Input<String>)> {
        let values = self.values(command)?;
        let options = if values.is_empty() {
            Input::Missing
        } else {
            values.into()
        };
        let config = self
            .native_config
            .as_deref()
            .map(|path| {
                Context::new(command, "--native-config")
                    .file(path)
                    .text(NATIVE_CONFIG_BYTES, true)
            })
            .transpose()?
            .map_or(Input::Missing, Input::Valid);
        Ok((options, config))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Controlled command and argument fixtures.
mod tests {
    use crate::cli::{Cli, Command};
    use clap::Parser;
    #[test]
    fn cli_options_are_typed_and_preset_does_not_collide_with_global_config() {
        let cli = Cli::try_parse_from([
            "hivectl",
            "spawn",
            "--preset",
            "reviewer",
            "--cwd",
            "/",
            "--body",
            "work",
            "--option",
            "effort=low",
            "--option",
            "codex.request_compression=false",
        ])
        .unwrap();
        let Command::Spawn(args) = cli.command else {
            panic!("spawn")
        };
        let crate::runtime::api::Request::Spawn(request) = args.request().unwrap() else {
            panic!("request")
        };
        assert_eq!(request.preset.as_str(), Some("reviewer"));
        assert_eq!(request.options.get().unwrap()["effort"], "low");
        assert_eq!(
            request.options.get().unwrap()["codex.request_compression"],
            false
        );
        assert!(Cli::try_parse_from(["hivectl", "set-model", "agent", "mock"]).is_err());
    }
    #[test]
    fn duplicate_assignments_and_missing_change_fail_without_silent_overrides() {
        let cli = Cli::try_parse_from([
            "hivectl",
            "set-options",
            "a",
            "--option",
            "effort=low",
            "--option",
            "effort=high",
        ])
        .unwrap();
        let Command::SetOptions(args) = cli.command else {
            panic!("set-options")
        };
        assert!(args.request().is_err());
        assert!(Cli::try_parse_from(["hivectl", "set-options", "a"]).is_err());
    }
}
