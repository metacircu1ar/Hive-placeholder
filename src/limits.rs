//! Shared bounds for accepted control-plane input.
// Idempotency keys are bounded before durable request lookup or insertion.
pub(crate) const REQUEST_ID_BYTES: usize = hive_input::validate::REQUEST_ID_BYTES;
// Bound each user-supplied control text field and each retained-output page to 64 KiB.
pub(crate) const CONTROL_INPUT_BYTES: usize = 64 * 1024;
// A maximum join is ~4 MiB of text; nested JSON escaping and headers need bounded headroom.
pub(crate) const JOIN_TRANSPORT_BYTES: usize = 64 * 1024 * 1024;
// Offline retention parameters must fit SQLite's signed integer domain exactly.
pub(crate) const COMPACT_BEFORE: std::ops::RangeInclusive<u64> = 0..=i64::MAX as u64;
pub(crate) const COMPACT_KEEP: std::ops::RangeInclusive<u64> = 1..=i64::MAX as u64;
