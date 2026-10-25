//! Search request bounds; page field bounds live with their field validators.
pub const DEFAULT_SEARCH_LIMIT: usize = 10;
pub const QUERY_BYTES: usize = 4096;
pub const SEARCH_LIMIT_MIN: usize = 1;
pub const SEARCH_LIMIT_MAX: usize = 100;

#[cfg(test)]
mod tests;
