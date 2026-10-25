//! Only pure scalar checks belong in clap; no model, environment or file access.
use comb::{Error, Result, embedding::HttpConfig};

pub(super) fn url(value: &str) -> Result<String> {
    HttpConfig::validate_url(value)?;
    Ok(value.into())
}

pub(super) fn identity(value: &str) -> Result<String> {
    if value.is_empty() || value.len() > HttpConfig::IDENTITY_BYTES {
        return Err(Error::invalid_embedding_space());
    }
    Ok(value.into())
}

pub(super) fn prefix(value: &str) -> Result<String> {
    if value.len() > HttpConfig::PREFIX_BYTES {
        return Err(Error::invalid_embedding_space());
    }
    Ok(value.into())
}
