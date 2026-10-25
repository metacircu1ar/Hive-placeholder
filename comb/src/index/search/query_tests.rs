use super::*;
use crate::{Comb, ErrorCode};

fn setup() -> (tempfile::TempDir, Comb, Search) {
    let root = tempfile::tempdir().unwrap();
    let comb = Comb::init(root.path()).unwrap();
    let request = Search {
        query: "query".into(),
        ..Search::default()
    };
    (root, comb, request)
}
fn unavailable() -> Error {
    let mut error = Error::new(ErrorCode::EmbeddingsBusy, "Original endpoint failure.")
        .caused_by(std::io::Error::from_raw_os_error(libc::EIO))
        .committed();
    error.request_id = Some("original-request".into());
    error
}

#[test]
fn vector_failures_retain_the_full_error_and_original_cause() {
    let (_root, comb, request) = setup();
    let expected = unavailable();
    let actual = comb
        .index()
        .unwrap()
        .retrieve(
            &request,
            Mode::Vector,
            QueryVector::Unavailable(expected.clone()),
        )
        .err()
        .unwrap();
    assert_eq!(
        crate::error::value(&actual).unwrap(),
        crate::error::value(expected).unwrap()
    );
    assert_eq!(
        crate::error::tests::io_cause(&actual).raw_os_error(),
        Some(libc::EIO)
    );
}

#[test]
fn not_requested_is_ignored_for_text_and_missing_for_vector() {
    let (_root, comb, request) = setup();
    let index = comb.index().unwrap();
    let text = crate::error::value(index.search(&request).unwrap()).unwrap();
    for query in [
        QueryVector::NotRequested,
        QueryVector::Unavailable(unavailable()),
    ] {
        assert_eq!(
            crate::error::value(index.retrieve(&request, Mode::Text, query).unwrap()).unwrap(),
            text
        );
    }
    let error = index
        .retrieve(&request, Mode::Vector, QueryVector::NotRequested)
        .err()
        .unwrap();
    assert_eq!(
        crate::error::value(error).unwrap(),
        crate::error::value(Error::query_model_missing()).unwrap()
    );
    let result = index
        .retrieve(&request, Mode::Hybrid, QueryVector::NotRequested)
        .unwrap();
    assert_eq!(result.mode, Mode::Text);
    assert_eq!(result.fallback, Some(Error::query_model_missing().code));
}

#[test]
fn hybrid_falls_back_for_ordinary_failures_but_preserves_recovery_errors() {
    let (_root, comb, request) = setup();
    let index = comb.index().unwrap();
    let result = index
        .retrieve(
            &request,
            Mode::Hybrid,
            QueryVector::Unavailable(unavailable()),
        )
        .unwrap();
    assert_eq!(result.mode, Mode::Text);
    assert_eq!(result.fallback, Some(ErrorCode::EmbeddingsBusy));
    assert!(result.coverage.is_some());
    let mut expected = Error::shared_state_poisoned().committed();
    expected.request_id = Some("committed-request".into());
    let error = index
        .retrieve(
            &request,
            Mode::Hybrid,
            QueryVector::Unavailable(expected.clone()),
        )
        .err()
        .unwrap();
    assert_eq!(
        crate::error::value(error).unwrap(),
        crate::error::value(expected).unwrap()
    );
}

#[test]
fn search_validation_still_precedes_all_query_outcomes() {
    let (_root, comb, mut request) = setup();
    request.limit = 0;
    for mode in [Mode::Text, Mode::Vector, Mode::Hybrid] {
        for query in [
            QueryVector::NotRequested,
            QueryVector::Unavailable(Error::shared_state_poisoned()),
        ] {
            let error = comb
                .index()
                .unwrap()
                .retrieve(&request, mode, query)
                .err()
                .unwrap();
            assert_eq!(
                crate::error::value(error).unwrap(),
                crate::error::value(Error::invalid_search()).unwrap()
            );
        }
    }
}
