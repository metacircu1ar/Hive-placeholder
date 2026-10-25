//! SQL owned by this store operation; statement text is a profiler key.

pub(super) const OPEN_SELECT: &str = "SELECT EXISTS(SELECT 1 FROM joins)";

#[cfg(feature = "comb")]
pub(super) const OPEN_SELECT_2: &str =
    "SELECT EXISTS(SELECT 1 FROM comb_collectors WHERE enabled = 1)";
