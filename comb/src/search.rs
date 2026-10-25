//! Filtered page-level results and query vectors.
pub use crate::index::search::QueryVector;
use crate::{
    ErrorCode,
    embedding::Coverage,
    values::{CreateSafety, MatchEvidence, Route, Slug, Zone},
};
pub use comb_protocol::search::{DEFAULT_SEARCH_LIMIT, Filter, Mode, Search, limits, validate};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hit {
    pub slug: Slug,
    pub title: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub score: f64,
    pub matched_by: Route,
    pub snippet: String,
    /// UTF-8 byte ranges for literal query words within the snippet.
    pub highlights: Vec<[usize; 2]>,
    pub heading: String,
    pub zone: Zone,
    pub evidence: MatchEvidence,
    pub create_safety: CreateSafety,
    pub unverified: bool,
    pub page_unverified: bool,
}
#[derive(Serialize, Deserialize)]
pub struct Results {
    pub results: Vec<Hit>,
    pub mode: Mode,
    pub fallback: Option<ErrorCode>,
    pub coverage: Option<Coverage>,
}
