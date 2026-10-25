use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct Cases {
    signed: Vec<(i64, u64)>,
    unsigned: Vec<(u64, u64)>,
    narrow: Vec<(u64, u32)>,
    saturating: Vec<(u64, u64)>,
}
fn cases() -> serde_json::Result<Cases> {
    serde_json::from_str(include_str!("baseline.json"))
}

#[test]
fn integer_rounding_matches_pre_refactor_bits() {
    let cases = cases().unwrap();
    for (input, bits) in cases.signed {
        assert_eq!(i64_f64(input).to_bits(), bits, "{input}");
    }
    for (input, bits) in cases.unsigned {
        if let Ok(input) = usize::try_from(input) {
            assert_eq!(usize_f64(input).to_bits(), bits, "{input}");
        }
    }
}

#[test]
fn narrowing_keeps_ties_signed_zero_subnormals_and_overflow() {
    for (input, bits) in cases().unwrap().narrow {
        assert_eq!(f64_f32(f64::from_bits(input)).to_bits(), bits, "{input:x}");
    }
}

#[test]
fn float_to_count_keeps_truncation_saturation_and_nan() {
    for (input, expected) in cases().unwrap().saturating {
        // The captured 64-bit ceiling becomes the platform ceiling on 32-bit.
        let expected = usize::try_from(expected).unwrap_or(usize::MAX);
        assert_eq!(
            f64_usize_saturating(f64::from_bits(input)),
            expected,
            "{input:x}"
        );
    }
}

#[test]
fn integer_caps_are_exact_at_validated_bounds_and_saturate_above_them() {
    for input in [0, 1, 32, 1000, 8192] {
        assert_eq!(usize_u64(input), u64::try_from(input).unwrap());
        assert_eq!(usize_u32_capped(input), u32::try_from(input).unwrap());
    }
    let max_u32 = usize::try_from(u32::MAX).unwrap();
    assert_eq!(usize_u32_capped(max_u32), u32::MAX);
    assert_eq!(usize_u32_capped(usize::MAX), u32::MAX);
    if let Some(above) = max_u32.checked_add(1) {
        assert_eq!(usize_u32_capped(above), u32::MAX);
    }
    assert_eq!(usize_u64(usize::MAX), u64::try_from(usize::MAX).unwrap());
}
