//! Frozen startup inputs compared with each observed daemon, including readiness.
use super::super::{CommandOutcome, Config, Error, Path, Result, api, input, runtime};
use crate::config::ConfigurationIdentity;
use hive_input::Code;
use serde::Deserialize;
use std::time::Instant;

pub(in crate::cli) struct Expected {
    binary_sha256: Option<String>,
    pub configuration: ConfigurationIdentity,
    providers: Option<runtime::ProviderSelection>,
    label: Option<String>,
}

#[derive(Deserialize)]
struct Snapshot {
    binary_sha256: String,
    protocols: hive_input::compatibility::ProtocolRange,
    #[serde(deserialize_with = "crate::runtime::domain::decode::object")]
    configuration: ConfigurationIdentity,
    #[serde(deserialize_with = "crate::runtime::domain::decode::object")]
    provider_selection: runtime::ProviderSelection,
    status: String,
    managed_service: Option<String>,
}

impl Expected {
    pub fn load(
        config: &Path,
        binaries: Option<&runtime::Binaries>,
        label: Option<String>,
    ) -> Result<Self> {
        Ok(Self {
            binary_sha256: if label.is_none() {
                Some(crate::executable::sha256(crate::executable::path()?)?)
            } else {
                None
            },
            configuration: Config::load(config)?
                .loaded
                .ok_or_else(|| Error::internal_error("Missing loaded configuration identity."))?,
            providers: binaries.map(runtime::Binaries::selection),
            label,
        })
    }

    pub fn program(mut self, path: &Path) -> Result<Self> {
        self.binary_sha256 = Some(crate::executable::sha256(path)?);
        Ok(self)
    }

    pub fn observe(&self, root: &Path, deadline: Instant) -> Result<Option<CommandOutcome>> {
        let rt = crate::calls::executor()?;
        let Some(value) =
            rt.block_on(runtime::client::Client::new(root).startup(deadline.into()))?
        else {
            return Ok(None);
        };
        self.check(value).map(Some)
    }

    pub(super) fn check(&self, value: serde_json::Value) -> Result<CommandOutcome> {
        if api::DaemonReply::new(&value)?.has_error() {
            return Ok(CommandOutcome {
                value,
                success: false,
                input: None,
            });
        }
        let snapshot: Snapshot = serde_json::from_value(value.clone())
            .map_err(|cause| runtime::client::envelope::invalid().with_source(cause))?;
        if snapshot.status != "running"
            || !snapshot.configuration.path.is_absolute()
            || [
                &snapshot.binary_sha256,
                &snapshot.configuration.sha256,
                &snapshot.provider_selection.codex,
                &snapshot.provider_selection.claude,
            ]
            .into_iter()
            .any(|digest| {
                digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        {
            return Err(runtime::client::envelope::invalid());
        }
        if crate::compatibility::CONTROL_RANGE
            .select(snapshot.protocols)
            .is_none()
        {
            return Err(Error::incompatible_protocol(
                "Running daemon has no supported control protocol in common.",
            ));
        }
        if self
            .binary_sha256
            .as_ref()
            .is_some_and(|expected| *expected != snapshot.binary_sha256)
        {
            return Err(Error::daemon_binary_mismatch(
                "The running daemon uses a different executable. Run hivectl daemon restart \
                    to use the selected build.",
            ));
        }
        if let Some(label) = &self.label
            && snapshot.managed_service.as_ref() != Some(label)
        {
            return Err(Error::config_invalid(
                "An unmanaged daemon is serving this state directory; stop it first.",
            ));
        }
        let field = if self.configuration != snapshot.configuration {
            Some("--config")
        } else if let Some(providers) = &self.providers {
            if providers.codex != snapshot.provider_selection.codex {
                Some("--codex-bin")
            } else if providers.claude != snapshot.provider_selection.claude {
                Some("--claude-bin")
            } else {
                None
            }
        } else {
            None
        };
        if let Some(field) = field {
            return Err(input::Context::new("daemon start", field)
                .invalid(
                    Code::ConflictingArguments,
                    "The running daemon uses different startup inputs. Inspect and stop it before \
                    retrying.",
                    "the running daemon's selection, or stop the daemon before starting with \
                    changed inputs",
                )
                .error);
        }
        Ok(CommandOutcome::ok(value))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
