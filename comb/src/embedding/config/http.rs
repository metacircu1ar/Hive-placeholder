//! Pure HTTP option rules shared with command-line preparation.
use super::HttpConfig;
use crate::{Error, Result, embedding::limits};
use std::ops::RangeInclusive;

impl HttpConfig {
    pub const DIMENSIONS: RangeInclusive<usize> = limits::DIMENSIONS_MIN..=limits::DIMENSIONS_MAX;
    pub const MAX_TOKENS: RangeInclusive<usize> = limits::TOKENS_MIN..=limits::TOKENS_MAX;
    pub const TIMEOUT_SECONDS: RangeInclusive<u64> =
        limits::HTTP_TIMEOUT_MIN_SECONDS..=limits::HTTP_TIMEOUT_MAX_SECONDS;
    pub const IDENTITY_BYTES: usize = limits::IDENTITY_BYTES;
    pub const PREFIX_BYTES: usize = limits::PREFIX_BYTES;

    /// Validate the endpoint and its normalized space identity without I/O.
    pub fn validate_url(value: &str) -> Result<()> {
        let url = endpoint(value, limits::DEFAULT_TIMEOUT_SECONDS)?;
        if super::super::identity::BackendIdentity::Http(&url)
            .encode()
            .len()
            > Self::IDENTITY_BYTES
        {
            return Err(Error::invalid_embedding_space());
        }
        Ok(())
    }
}

pub(crate) fn endpoint(url: &str, timeout_seconds: u64) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(url)
        .map_err(|cause| Error::invalid_embedding_url().caused_by(cause))?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
        || !HttpConfig::TIMEOUT_SECONDS.contains(&timeout_seconds)
    {
        return Err(Error::unsafe_embedding_endpoint());
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn endpoint_checks_normalized_identity_bytes_and_retains_parser_cause() {
        let base = "https://example.test/";
        let full = format!(
            "{base}{}",
            "x".repeat(HttpConfig::IDENTITY_BYTES - base.len() - 5)
        );
        assert!(HttpConfig::validate_url(&full).is_ok());
        assert!(HttpConfig::validate_url(&format!("{full}x")).is_err());
        // URL escaping can exceed the bound even when the caller string fits.
        let escaped = format!("{base}{}", "é".repeat(1000));
        assert!(escaped.len() < HttpConfig::IDENTITY_BYTES);
        assert!(HttpConfig::validate_url(&escaped).is_err());
        let error = HttpConfig::validate_url("not a URL").unwrap_err();
        assert!(error.source().is_some());
        let encoded = serde_json::to_string(&error).unwrap();
        assert!(!encoded.contains("not a URL"));
    }
}
