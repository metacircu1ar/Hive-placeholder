use super::{Login, Result, Values, removal};
use crate::cli::input::Context;
use crate::config::Provider;
use hive_input::{Code, validate};

impl Values {
    pub(super) fn validate_supplied(&mut self) -> Result<()> {
        self.provider_choices()?;
        for (field, value, rule) in [
            (
                "--codex-profile",
                &self.codex_profile,
                validate::config_name as fn(&str) -> _,
            ),
            (
                "--claude-profile",
                &self.claude_profile,
                validate::config_name,
            ),
            ("--codex-model", &self.codex_model, validate::model),
            ("--claude-model", &self.claude_model, validate::model),
            ("--codex-effort", &self.codex_effort, validate::effort),
            ("--claude-effort", &self.claude_effort, validate::effort),
        ] {
            if let Some(value) = value {
                rule(value).map_err(|cause: validate::Violation| {
                    Context::new("setup", field)
                        .invalid(cause.code(), "Invalid setup option.", cause.expected())
                        .with_source(cause)
                        .error
                })?;
            }
        }
        if let Some(value) = &self.service_path {
            self.service_path = Some(
                hive_setup_support::environment::service_path(Some(value)).map_err(|cause| {
                    Context::new("setup", "--service-path")
                        .invalid(
                            Code::InvalidValue,
                            "Invalid service search path.",
                            "a colon-separated list of absolute existing directories without \
                                control characters or empty entries",
                        )
                        .with_source(cause)
                        .error
                })?,
            );
        }
        Ok(())
    }

    fn provider_choices(&self) -> Result<()> {
        let Some(providers) = &self.provider else {
            return Ok(());
        };
        if providers.is_empty()
            || providers
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != providers.len()
        {
            return Err(removal::invalid(
                "--provider",
                "Invalid provider selection.",
                "unique codex/claude choices, at least one",
            ));
        }
        if self
            .default_provider
            .is_some_and(|provider| !providers.contains(&provider))
        {
            return Err(removal::invalid(
                "--default-provider",
                "Default provider is not selected.",
                "a provider included in --provider",
            ));
        }
        if self.login == Some(Login::Device) && providers.contains(&Provider::Claude) {
            return Err(removal::invalid(
                "--login",
                "Device login is Codex-only.",
                "browser or skip when Claude is selected",
            ));
        }
        Ok(())
    }
}
