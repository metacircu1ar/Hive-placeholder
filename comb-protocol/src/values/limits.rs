//! Identity byte bounds and filing recency policies.
pub const SLUG_BYTES: usize = 240;
pub const SLUG_COMPONENT_BYTES: usize = 100;
pub use hive_input::validate::REQUEST_ID_BYTES;
pub const FIELD_BYTES: usize = 512;
pub const STABLE_HALF_LIFE_DAYS: f64 = 0.0;
pub const SHORT_HALF_LIFE_DAYS: f64 = 7.0;
pub const ORDINARY_HALF_LIFE_DAYS: f64 = 90.0;
