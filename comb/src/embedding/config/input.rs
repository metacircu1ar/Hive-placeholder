//! Explicit caller fields are checked without assets, runtime or credentials.
use super::{Config, HttpConfig};
use crate::{Error, Result, embedding::limits};
use hive_input::{Code, InputError};

impl Config {
    pub fn validate_input(&self) -> Result<()> {
        let Self::Http(config) = self;
        http(config)
    }
}
fn rule(valid: bool, field: &str, expected: &str, cause: impl FnOnce() -> Error) -> Result<()> {
    if valid {
        Ok(())
    } else {
        Err(invalid(Code::InvalidValue, field, expected, cause()))
    }
}
fn text(value: &str, field: &str, nonempty: bool, limit: usize, expected: &str) -> Result<()> {
    rule(
        (!nonempty || !value.is_empty()) && value.len() <= limit,
        field,
        expected,
        Error::invalid_embedding_space,
    )
}
fn http(config: &HttpConfig) -> Result<()> {
    for (field, value) in [("model", &config.model), ("revision", &config.revision)] {
        text(
            value,
            field,
            true,
            HttpConfig::IDENTITY_BYTES,
            "1..4096 UTF-8 bytes",
        )?;
    }
    rule(
        HttpConfig::DIMENSIONS.contains(&config.dimensions),
        "dimensions",
        "an integer within 1..4096",
        Error::invalid_embedding_space,
    )?;
    prefixes([
        Some(config.document_prefix.as_str()),
        Some(config.query_prefix.as_str()),
    ])?;
    rule(
        HttpConfig::TIMEOUT_SECONDS.contains(&config.timeout_seconds),
        "timeout_seconds",
        "an integer within 1..120 seconds",
        Error::unsafe_embedding_endpoint,
    )?;
    HttpConfig::validate_url(&config.url).map_err(|cause| {
        invalid(
            Code::InvalidValue,
            "url",
            "an HTTPS endpoint or loopback HTTP URL without user-info, query or \
            fragment; normalized http:URL identity at most 4096 bytes",
            cause,
        )
    })?;
    match (&config.tokenizer, config.max_tokens) {
        (None, None) => Ok(()),
        (Some(path), Some(limit)) => {
            rule(
                !path.as_os_str().is_empty(),
                "tokenizer",
                "a nonempty tokenizer path",
                Error::embedding_asset_path_required,
            )?;
            rule(
                HttpConfig::MAX_TOKENS.contains(&limit),
                "max_tokens",
                "an integer within 16..8192, together with tokenizer",
                Error::http_tokenizer_token_limit,
            )
        }
        (Some(_), None) => Err(invalid(
            Code::MissingArgument,
            "max_tokens",
            "max_tokens together with tokenizer",
            Error::http_tokenizer_pair_required(),
        )),
        (None, Some(_)) => Err(invalid(
            Code::MissingArgument,
            "tokenizer",
            "tokenizer together with max_tokens",
            Error::http_tokenizer_pair_required(),
        )),
    }
}
fn prefixes(values: [Option<&str>; 2]) -> Result<()> {
    for (field, value) in ["document_prefix", "query_prefix"].into_iter().zip(values) {
        if let Some(value) = value {
            text(
                value,
                field,
                false,
                limits::PREFIX_BYTES,
                "at most 1024 UTF-8 bytes; empty and multiline prefixes allowed",
            )?;
        }
    }
    Ok(())
}
fn invalid(code: Code, field: &str, expected: &str, cause: Error) -> Error {
    match InputError::new(
        code,
        "Invalid embedding configuration input.",
        format!("/configuration/{field}"),
        format!("configuration field {field}: {expected}"),
        vec!["comb embed --config embeddings.yaml".into()],
    ) {
        Ok(input) => Error::from(input).caused_by(cause),
        Err(error) => Error::internal_serialization().caused_by(error),
    }
}

#[cfg(test)]
mod tests;
