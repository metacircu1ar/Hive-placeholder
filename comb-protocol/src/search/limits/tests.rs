use crate::{
    ErrorCode,
    search::{Search, validate},
};

#[test]
fn query_ceiling_counts_bytes_and_search_limit_keeps_both_endpoints() {
    let query = "é".repeat(2048);
    for limit in [1, 100] {
        let request = Search {
            query: query.clone(),
            limit,
            ..Search::default()
        };
        validate(&request).unwrap();
        let too_long = Search {
            query: format!("{query}x"),
            ..request
        };
        assert_eq!(
            validate(&too_long).unwrap_err().code,
            ErrorCode::InvalidSearch
        );
    }
    for limit in [0, 101, usize::MAX] {
        let request = Search {
            query: query.clone(),
            limit,
            ..Search::default()
        };
        assert_eq!(
            validate(&request).unwrap_err().code,
            ErrorCode::InvalidSearch
        );
    }
}

#[test]
fn filters_and_queries_count_utf8_bytes_and_dates_compare_instants() {
    use crate::{
        limits::{KIND_BYTES, LABEL_BYTES},
        values::limits::SLUG_BYTES,
    };
    for max in [KIND_BYTES, LABEL_BYTES, SLUG_BYTES] {
        let text = "é".repeat(max / 2);
        super::super::filter_text(&text, max).unwrap();
        assert!(super::super::filter_text(&format!("{text}x"), max).is_err());
        assert!(super::super::filter_text("", max).is_err());
    }
    assert!(super::super::query(" \n ").is_err());
    let at = |value: &str| value.parse().unwrap();
    for (after, before, valid) in [
        ("2026-01-01T01:00:00+01:00", "2026-01-01T00:00:00Z", true),
        ("2026-01-01T01:00:01+01:00", "2026-01-01T00:00:00Z", false),
        ("2026-01-01T00:00:00Z", "2026-01-01T01:00:00-01:00", true),
    ] {
        let request = Search {
            query: "test".into(),
            after: Some(at(after)),
            before: Some(at(before)),
            ..Search::default()
        };
        assert_eq!(validate(&request).is_ok(), valid);
    }
    let bad = crate::request::List {
        after: Some("invalid cursor".into()),
        ..Default::default()
    };
    assert!(bad.validate().is_err());
    for filter in [
        crate::request::List {
            kind: Some(String::new()),
            ..Default::default()
        },
        crate::request::List {
            tag: Some(String::new()),
            ..Default::default()
        },
        crate::request::List {
            prefix: Some(String::new()),
            ..Default::default()
        },
    ] {
        assert!(filter.validate().is_err());
    }
}
