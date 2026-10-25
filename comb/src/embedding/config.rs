//! Operator settings for an HTTP embedding endpoint.
pub(crate) mod http;
mod input;
use super::limits;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "backend",
    rename_all = "snake_case",
    deny_unknown_fields,
    from = "raw::Config"
)]
pub enum Config {
    Http(HttpConfig),
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    pub model: String,
    /// Immutable revision supplied by the endpoint operator.
    pub revision: String,
    pub url: String,
    pub dimensions: usize,
    #[serde(default)]
    pub document_prefix: String,
    #[serde(default)]
    pub query_prefix: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default = "deadline")]
    pub timeout_seconds: u64,
    /// Send the optional `OpenAI` dimensions parameter only when requested.
    #[serde(default)]
    pub request_dimensions: bool,
    #[serde(default)]
    pub tokenizer: Option<PathBuf>,
    #[serde(default)]
    pub max_tokens: Option<usize>,
}

// Serde's diagnostics use the Rust type name, including positional-sequence errors.
// Keep the current raw tagged shape and name while the public representation splits.
mod raw {
    use super::{Deserialize, PathBuf, deadline};

    #[derive(Deserialize)]
    #[serde(tag = "backend", rename_all = "snake_case", deny_unknown_fields)]
    pub(super) enum Config {
        Http {
            model: String,
            /// Immutable revision supplied by the endpoint operator.
            revision: String,
            url: String,
            dimensions: usize,
            #[serde(default)]
            document_prefix: String,
            #[serde(default)]
            query_prefix: String,
            #[serde(default)]
            api_key_env: Option<String>,
            #[serde(default = "deadline")]
            timeout_seconds: u64,
            /// Send the optional `OpenAI` dimensions parameter only when requested.
            #[serde(default)]
            request_dimensions: bool,
            #[serde(default)]
            tokenizer: Option<PathBuf>,
            #[serde(default)]
            max_tokens: Option<usize>,
        },
    }
}

impl From<raw::Config> for Config {
    fn from(config: raw::Config) -> Self {
        match config {
            raw::Config::Http {
                model,
                revision,
                url,
                dimensions,
                document_prefix,
                query_prefix,
                api_key_env,
                timeout_seconds,
                request_dimensions,
                tokenizer,
                max_tokens,
            } => Self::Http(HttpConfig {
                model,
                revision,
                url,
                dimensions,
                document_prefix,
                query_prefix,
                api_key_env,
                timeout_seconds,
                request_dimensions,
                tokenizer,
                max_tokens,
            }),
        }
    }
}
fn deadline() -> u64 {
    limits::DEFAULT_TIMEOUT_SECONDS
}
impl Config {
    pub fn read(path: &Path) -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?;
        if !file.metadata()?.is_file() {
            return Err(Error::invalid_embedding_config_file());
        }
        let bytes = crate::bounded::bytes(
            file,
            limits::CONFIG_BYTES,
            Error::invalid_embedding_config_file,
        )?;
        let absolute = std::path::absolute(path)?;
        Self::parse(&bytes, absolute.parent())
    }
    /// Decode once-read config bytes. Pipes have no base for relative assets.
    pub fn parse(bytes: &[u8], base: Option<&Path>) -> Result<Self> {
        if bytes.len() > limits::CONFIG_BYTES {
            return Err(Error::invalid_embedding_config_file());
        }
        let mut config: Self = serde_yaml_ng::from_slice(bytes)
            .map_err(|cause| Error::invalid_embedding_config_file().caused_by(cause))?;
        let Self::Http(http) = &mut config;
        if let Some(path) = http.tokenizer.as_mut() {
            if path.as_os_str().is_empty() {
                return Err(Error::embedding_asset_path_required());
            }
            if path.is_relative() {
                let base = base
                    .filter(|base| base.is_absolute())
                    .ok_or_else(Error::embedding_asset_base_required)?;
                *path = base.join(&path);
            }
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests;
