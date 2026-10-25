use super::*;

#[test]
fn gc_duration_requires_explicit_positive_bounded_units() {
    assert_eq!(parse("30d"), Ok(2_592_000));
    assert_eq!(parse("2w"), Ok(1_209_600));
    for bad in [
        "",
        "0s",
        "30",
        "-1d",
        "+1h",
        "1.5h",
        "1D",
        "1d2h",
        " 1s",
        "∞",
        "18446744073709551615w",
        "9223372036854775808s",
    ] {
        assert!(parse(bad).is_err(), "{bad}");
    }
}
