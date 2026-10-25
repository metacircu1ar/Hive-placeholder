//! Pure CLI adapters for the serving selection.
use super::Result;

/// Selection for `model use`: only text search remains a named choice.
pub fn choice(value: &str) -> Result<String> {
    if value == "none" {
        Ok(value.into())
    } else {
        Err(super::invalid(
            "Only `none` (text search) can be selected by name; use `model use-http` for \
                embeddings.",
        ))
    }
}

pub fn prefix(value: &str) -> Result<String> {
    if value.len() > crate::embedding::limits::PREFIX_BYTES {
        return Err(crate::Error::invalid_embedding_space());
    }
    Ok(value.into())
}
