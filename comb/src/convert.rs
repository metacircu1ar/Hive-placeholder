//! Exact conversion policies; floating casts intentionally retain Rust's rounding.

pub(crate) fn usize_u32_capped(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "preserve the existing integer-to-f64 rounding"
)]
pub(crate) fn usize_f64(value: usize) -> f64 {
    value as f64
}

#[expect(
    clippy::cast_precision_loss,
    reason = "preserve the existing integer-to-f64 rounding"
)]
pub(crate) fn i64_f64(value: i64) -> f64 {
    value as f64
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "one deliberate f64-to-f32 rounding after accumulation"
)]
pub(crate) fn f64_f32(value: f64) -> f32 {
    value as f32
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "preserve Rust's truncating and saturating float conversion"
)]
#[expect(
    clippy::cast_sign_loss,
    reason = "negative values and NaN saturate to zero"
)]
pub(crate) fn f64_usize_saturating(value: f64) -> usize {
    value as usize
}

#[cfg(test)]
mod tests;

pub(crate) use comb_protocol::convert::usize_u64;
