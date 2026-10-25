//! Typed wire payloads with no store implementation.
use crate::values::{ContentHash, Slug};
use serde::{Deserialize, Serialize};
#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct List {
    pub kind: Option<String>,
    pub tag: Option<String>,
    pub prefix: Option<String>,
    pub unverified: bool,
    pub after: Option<String>,
    pub limit: Option<usize>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Consolidate {
    pub note: Slug,
    pub target: Slug,
    pub note_hash: ContentHash,
    pub target_hash: ContentHash,
    pub summary: String,
    #[serde(default)]
    pub recopy: bool,
}

impl List {
    pub fn validate(&self) -> crate::Result<()> {
        use crate::limits::{DEFAULT_LIST_LIMIT, LIST_LIMIT_MAX, LIST_LIMIT_MIN};
        if !(LIST_LIMIT_MIN..=LIST_LIMIT_MAX).contains(&self.limit.unwrap_or(DEFAULT_LIST_LIMIT)) {
            return Err(crate::Error::invalid_list_limit());
        }
        crate::search::Filter {
            kind: self.kind.as_deref(),
            tag: self.tag.as_deref(),
            prefix: self.prefix.as_deref(),
            ..crate::search::Filter::default()
        }
        .validate()?;
        if let Some(after) = &self.after {
            after.parse::<Slug>()?;
        }
        Ok(())
    }
}
