//! Embedding policy bounds, with separate names for distinct resource budgets.
pub const BATCH_SIZE: usize = 32;
pub const INPUT_BYTES: usize = 64 * 1024;

pub(crate) const DIMENSIONS_MIN: usize = 1;
pub(crate) const DIMENSIONS_MAX: usize = 4096;
pub(super) const IDENTITY_BYTES: usize = 4096;
pub(crate) const PREFIX_BYTES: usize = 1024;
pub(super) const UNIT_NORM_TOLERANCE: f64 = 0.0001;
pub const CONFIG_BYTES: usize = 16 * 1024;
pub(super) const DEFAULT_TIMEOUT_SECONDS: u64 = 30;

pub(super) const HTTP_TIMEOUT_MIN_SECONDS: u64 = 1;
pub(super) const HTTP_TIMEOUT_MAX_SECONDS: u64 = 120;
pub(super) const HTTP_CONNECT_SECONDS: u64 = 10;
pub(super) const HTTP_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const TOKENS_MIN: usize = 16;
pub(crate) const TOKENS_MAX: usize = 8192;
pub(crate) const TOKENIZER_BYTES: usize = 64 * 1024 * 1024;

pub(super) const BACKFILL_MIN_BATCH: usize = 1;
pub(super) const BACKFILL_TARGET_SECONDS: f64 = 0.2;
pub(super) const BACKFILL_TIME_FLOOR_SECONDS: f64 = 0.000_001;
pub(super) const BACKFILL_OLD_WEIGHT: f64 = 0.75;
pub(super) const BACKFILL_SAMPLE_WEIGHT: f64 = 0.25;

#[cfg(test)]
mod tests;
