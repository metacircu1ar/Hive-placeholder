use super::*;
use crate::{ErrorCode, test_support::assert_code};

fn space() -> Space {
    Space {
        backend: "fixture".into(),
        model: "fixture".into(),
        revision: "fixture".into(),
        dimensions: 2,
        document_prefix: "d:".into(),
        query_prefix: "q:".into(),
        preprocessing: "fixture".into(),
        normalization: "l2".into(),
    }
}

#[test]
fn ordered_row_results_keep_rejections_separate_from_batch_failure() {
    let mut engine = FakeEmbedder::new(space(), |input| match input {
        "d:first" => Ok(vec![1.0, 0.0]),
        "d:rejected" => Err(Error::new(ErrorCode::EmbeddingInputRejected, "row detail")),
        "d:last" => Ok(vec![0.0, 1.0]),
        _ => panic!("unexpected fixture input"),
    });
    let rows = engine
        .embed(&["d:first".into(), "d:rejected".into(), "d:last".into()])
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].as_ref().unwrap(), &[1.0, 0.0]);
    let error = assert_code!(rows[1].clone(), ErrorCode::EmbeddingInputRejected);
    assert_eq!(error.message, "row detail");
    assert_eq!(rows[2].as_ref().unwrap(), &[0.0, 1.0]);
    assert_eq!(engine.calls().load(Ordering::SeqCst), 1);
    assert_eq!(engine.input_count(), 3);
    assert_eq!(engine.space(), &space());
    assert!(engine.query_engine().is_none());
}

#[test]
fn batch_failure_preserves_cause_and_metadata_before_query_observation_and_rows() {
    let mut expected = Error::new(ErrorCode::FixtureUnavailable, "batch detail")
        .caused_by(std::io::Error::from_raw_os_error(libc::EIO))
        .committed();
    expected.request_id = Some("fixture-request".into());
    let mut engine = FakeEmbedder::new(space(), |_| panic!("failed batch cannot produce rows"))
        .batch_error(expected.clone());
    let error = assert_code!(
        engine.embed(&["q:first".into(), "q:second".into()]),
        ErrorCode::FixtureUnavailable
    );
    assert_eq!(
        crate::error::value(&error).unwrap(),
        crate::error::value(&expected).unwrap()
    );
    assert_eq!(
        crate::error::tests::io_cause(&error).raw_os_error(),
        Some(libc::EIO)
    );
    assert_eq!(engine.calls().load(Ordering::SeqCst), 1);
    assert_eq!(engine.input_count(), 2);
    assert_eq!(engine.query_calls().load(Ordering::SeqCst), 0);
}
