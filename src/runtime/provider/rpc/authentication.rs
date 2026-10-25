//! Prove the selected method before native thread work, without persisting login replies.
use super::{Error, Result, Rpc, outgoing};
use crate::{auth::Credential, config::Method};
use serde_json::Value;
use std::path::Path;

impl Rpc {
    pub(crate) fn initialize_authenticated(
        &mut self,
        credential: &Credential,
        home: &Path,
    ) -> Result<()> {
        self.expected_method = credential.method();
        self.private_bootstrap = self.expected_method == Method::ApiKey;
        let result = (|| {
            self.initialize()?;
            if self.claude.is_none() && self.expected_method == Method::ApiKey {
                let requirements = self.call(outgoing::config_requirements())?.into_raw();
                let config = self.call(outgoing::config_read())?.into_raw();
                if !safe_requirements(&requirements) || !safe_config(&config) {
                    return Err(Error::policy_conflict(
                        "Codex must prove ephemeral storage and the built-in OpenAI provider \
                            before API login.",
                    ));
                }
                ephemeral_home(home)?;
                let response = self
                    .call(outgoing::OutgoingRequest::ApiLogin(credential.login()?))?
                    .into_raw();
                if response.get("type").and_then(Value::as_str) != Some("apiKey") {
                    return Err(Error::authentication_required(
                        "Codex did not acknowledge the selected API authentication method.",
                    ));
                }
            }
            self.validate_account()?;
            if self.claude.is_none() && self.expected_method == Method::ApiKey {
                ephemeral_home(home)?;
            }
            Ok(())
        })();
        if self.private_bootstrap {
            self.pending = super::ByteQueue::default();
        }
        self.private_bootstrap = false;
        result
    }
    pub(crate) fn validate_account(&mut self) -> Result<()> {
        let valid = if let Some(codec) = &mut self.claude {
            let valid = codec.check_method(self.expected_method);
            self.account_identity = (self.expected_method == Method::Subscription)
                .then(|| codec.account_identity().map(str::to_owned))
                .flatten();
            valid
        } else {
            let account = self.call(outgoing::account_read())?;
            self.account_identity = (self.expected_method == Method::Subscription)
                .then(|| account.account_identity().map(str::to_owned))
                .flatten();
            match self.expected_method {
                Method::Subscription => account.subscription_account(),
                Method::ApiKey => account.api_account(),
            }
        };
        if valid {
            Ok(())
        } else {
            Err(Error::authentication_required(
                "Provider account does not match the selected authentication method.",
            ))
        }
    }
}

fn ephemeral_home(home: &Path) -> Result<()> {
    match std::fs::symlink_metadata(home.join("auth.json")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::state_io_from(error)),
        Ok(_) => Err(Error::policy_conflict(
            "Codex API login requires an isolated home without persisted credentials; \
                retain the home for inspection.",
        )),
    }
}

fn optional_selection(value: Option<&Value>, selected: &str) -> bool {
    value.is_none_or(|value| value.is_null() || value.as_str() == Some(selected))
}
fn safe_requirements(value: &Value) -> bool {
    let Some(requirements) = value.get("requirements") else {
        return false;
    };
    requirements.is_null()
        || requirements.is_object()
            && optional_selection(requirements.get("cliAuthCredentialsStore"), "ephemeral")
            && optional_selection(requirements.get("modelProvider"), "openai")
            && requirements.get("allowedLoginMethods").is_none_or(|value| {
                value.is_null()
                    || value.as_array().is_some_and(|methods| {
                        methods.iter().all(Value::is_string)
                            && methods.iter().any(|method| method.as_str() == Some("api"))
                    })
            })
            && requirements.get("modelProviders").is_none_or(|value| {
                value.is_null()
                    || value
                        .as_object()
                        .is_some_and(|map| !map.contains_key("openai"))
            })
}
fn safe_config(value: &Value) -> bool {
    let Some(config) = value.get("config").filter(|v| v.is_object()) else {
        return false;
    };
    config
        .get("cli_auth_credentials_store")
        .and_then(Value::as_str)
        == Some("ephemeral")
        && config.get("model_provider").and_then(Value::as_str) == Some("openai")
        && optional_selection(config.get("openai_base_url"), "https://api.openai.com/v1")
        && optional_selection(config.get("forced_login_method"), "api")
        && config.get("model_providers").is_none_or(|value| {
            value.is_null()
                || value
                    .as_object()
                    .is_some_and(|map| !map.contains_key("openai"))
        })
}
