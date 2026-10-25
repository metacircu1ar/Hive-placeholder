//! Authored page and evidence bounds, measured in bytes unless named otherwise.
pub const PAGE_BYTES: usize = 1024 * 1024;
pub(crate) const TITLE_BYTES: usize = 1024;
pub(crate) const TAG_COUNT: usize = 64;
pub(crate) const ALIAS_COUNT: usize = 64;
pub const SOURCE_BYTES: usize = 4096;
pub(super) const CONSOLIDATION_TARGETS: usize = 128;

#[cfg(test)]
mod tests;

pub(crate) use comb_protocol::limits::{KIND_BYTES, LABEL_BYTES};
