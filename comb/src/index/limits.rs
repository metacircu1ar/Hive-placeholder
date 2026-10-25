//! Independent listing, traversal, cache and search presentation policies.
// The immutable degree SQL reads at most this bound plus one sentinel neighbor.
pub(crate) const GRAPH_PAGE_LIMIT: u16 = 1000;
pub(super) const CACHE_ENTRY_OVERHEAD_BYTES: usize = 256;
pub(super) const IDENTITY_LONG_QUERY_CHARS: usize = 3;
pub(super) const HEADING_CHARS: usize = 1024;
pub(super) const SNIPPET_CONTEXT_CHARS: usize = 80;
pub(super) const SNIPPET_CHARS: usize = 480;
pub(super) const RRF_K: f64 = 60.0;
pub(super) const COMPILED_MULTIPLIER: f64 = 1.1;
pub(super) const TITLE_MULTIPLIER: f64 = 1.1;
pub(super) const RECENCY_BONUS: f64 = 0.1;
pub(super) const SECONDS_PER_DAY: f64 = 86400.0;

#[cfg(test)]
mod tests;

pub(crate) use comb_protocol::limits::{
    DEFAULT_LIST_LIMIT, GRAPH_DEPTH_MAX, LINK_LIMIT_MAX, LINK_LIMIT_MIN,
};
