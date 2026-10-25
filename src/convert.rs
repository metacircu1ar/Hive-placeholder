//! Numeric representations used by storage and OS boundaries.

/// Check an unsigned timestamp or byte count before storing it in SQLite.
pub(crate) fn sqlite_i64(value: u64) -> Result<i64, std::num::TryFromIntError> {
    i64::try_from(value)
}

/// Reject a negative SQLite value when the stored quantity is unsigned.
pub(crate) fn unsigned_u64(value: i64) -> Result<u64, std::num::TryFromIntError> {
    u64::try_from(value)
}

/// Preserve the 32-bit representation when passing flags or PIDs to signed C APIs.
pub(crate) fn bits_i32(value: u32) -> i32 {
    i32::from_ne_bytes(value.to_ne_bytes())
}

/// Cap an unsigned query bound at SQLite's largest signed integer.
pub(crate) fn capped_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Reject negative counts and counts that do not fit this platform.
pub(crate) fn count_usize(value: i64) -> Result<usize, std::num::TryFromIntError> {
    usize::try_from(value)
}

#[cfg(test)]
mod tests {
    use super::{bits_i32, capped_i64, count_usize, sqlite_i64, unsigned_u64};

    #[test]
    fn storage_integers_reject_wrapping_and_native_i32_preserves_bits() {
        for value in [0, 1, i64::MAX] {
            assert_eq!(sqlite_i64(value.unsigned_abs()), Ok(value));
            assert_eq!(unsigned_u64(value), Ok(value.unsigned_abs()));
        }
        for value in [i64::MAX.unsigned_abs() + 1, u64::MAX] {
            assert!(sqlite_i64(value).is_err());
        }
        for value in [-1, i64::MIN] {
            assert!(unsigned_u64(value).is_err());
        }
        assert_eq!(bits_i32(0), 0);
        assert_eq!(bits_i32(0x7fff_ffff), i32::MAX);
        assert_eq!(bits_i32(0x8000_0000), i32::MIN);
        assert_eq!(bits_i32(u32::MAX), -1);
    }

    #[test]
    fn query_bounds_cap_without_wrapping() {
        assert_eq!(capped_i64(0), 0);
        assert_eq!(capped_i64(200), 200);
        assert_eq!(capped_i64(u64::from(u32::MAX)), i64::from(u32::MAX));
        assert_eq!(capped_i64(0x7fff_ffff_ffff_fffe), i64::MAX - 1);
        assert_eq!(capped_i64(0x7fff_ffff_ffff_ffff), i64::MAX);
        assert_eq!(capped_i64(0x8000_0000_0000_0000), i64::MAX);
        assert_eq!(capped_i64(u64::MAX), i64::MAX);
    }

    #[test]
    fn counts_require_nonnegative_platform_representable_values() {
        assert_eq!(count_usize(0), Ok(0));
        assert_eq!(count_usize(200), Ok(200));
        assert!(count_usize(-1).is_err());
        assert!(count_usize(-2).is_err());
        assert!(count_usize(i64::MIN).is_err());
        #[cfg(target_pointer_width = "32")]
        {
            assert_eq!(count_usize(0xffff_ffff), Ok(usize::MAX));
            assert!(count_usize(0x1_0000_0000).is_err());
            assert!(count_usize(0x1_0000_0001).is_err());
            assert!(count_usize(i64::MAX).is_err());
        }
        #[cfg(target_pointer_width = "64")]
        assert_eq!(count_usize(i64::MAX), Ok(0x7fff_ffff_ffff_ffff));
    }
}
