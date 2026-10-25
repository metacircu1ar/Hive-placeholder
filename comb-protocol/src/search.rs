//! Validated retrieval request payloads.
use crate::values::limits::SLUG_BYTES;
use crate::{
    Error, Result,
    limits::{KIND_BYTES, LABEL_BYTES},
    values::Timestamp,
};
use serde::{Deserialize, Serialize};
pub mod limits;
pub use limits::DEFAULT_SEARCH_LIMIT;
use limits::{QUERY_BYTES, SEARCH_LIMIT_MAX, SEARCH_LIMIT_MIN};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Search {
    pub query: String,
    pub limit: usize,
    pub kind: Option<String>,
    pub tag: Option<String>,
    pub prefix: Option<String>,
    pub after: Option<Timestamp>,
    pub before: Option<Timestamp>,
}
impl Default for Search {
    fn default() -> Self {
        Self {
            query: String::new(),
            limit: DEFAULT_SEARCH_LIMIT,
            kind: None,
            tag: None,
            prefix: None,
            after: None,
            before: None,
        }
    }
}
impl Search {
    pub fn filter(&self) -> Filter<'_> {
        Filter {
            kind: self.kind.as_deref(),
            tag: self.tag.as_deref(),
            prefix: self.prefix.as_deref(),
            after: self.after.as_ref(),
            before: self.before.as_ref(),
        }
    }
}

/// Borrowed page filters shared by retrieval and embedding coverage.
#[derive(Clone, Copy, Debug, Default)]
pub struct Filter<'a> {
    pub kind: Option<&'a str>,
    pub tag: Option<&'a str>,
    pub prefix: Option<&'a str>,
    pub after: Option<&'a Timestamp>,
    pub before: Option<&'a Timestamp>,
}
impl Filter<'_> {
    pub fn validate(self) -> Result<()> {
        for (value, bound) in [
            (self.kind, KIND_BYTES),
            (self.tag, LABEL_BYTES),
            (self.prefix, SLUG_BYTES),
        ] {
            if let Some(value) = value {
                filter_text(value, bound)?;
            }
        }
        date_range(self.after, self.before)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Text,
    Vector,
    #[default]
    Hybrid,
}

pub fn validate(request: &Search) -> Result<()> {
    query(&request.query)?;
    if !(SEARCH_LIMIT_MIN..=SEARCH_LIMIT_MAX).contains(&request.limit) {
        return Err(Error::invalid_search());
    }
    request.filter().validate()
}

/// A literal metadata filter; unlike a slug, a prefix may end in a slash.
pub fn filter_text(value: &str, max_bytes: usize) -> Result<()> {
    if value.is_empty() || value.len() > max_bytes {
        return Err(Error::invalid_search());
    }
    Ok(())
}

pub fn query(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > QUERY_BYTES {
        return Err(Error::invalid_query());
    }
    Ok(())
}

/// Compare instants, not the lexical spellings of timezone offsets.
pub fn date_range(after: Option<&Timestamp>, before: Option<&Timestamp>) -> Result<()> {
    if let (Some(after), Some(before)) = (after, before) {
        let parse = |value: &Timestamp| {
            chrono::DateTime::parse_from_rfc3339(value.as_str())
                .map_err(|cause| Error::invalid_search_dates().caused_by(cause))
        };
        if parse(after)? > parse(before)? {
            return Err(Error::reversed_search_dates());
        }
    }
    Ok(())
}
