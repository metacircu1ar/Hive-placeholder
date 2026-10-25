//! Transport regressions.
use super::*;

#[tokio::test]
async fn committed_reply_and_publication_do_not_wait_for_idle_checkpoint() {
    reply_and_publication_precede_checkpoint(false).await;
}

#[tokio::test]
async fn committed_reply_and_publication_do_not_wait_for_deferred_checkpoint() {
    reply_and_publication_precede_checkpoint(true).await;
}

#[tokio::test]
async fn rejects_mutations_and_nonhuman_callers_and_releases_error_snapshots() {
    let runtime = RuntimeFixture::new();
    let started = start(&runtime).await;
    for request in [
        json!({"op":"stop", "target":"agent"}),
        json!({"op":"batch", "operations":[]}),
    ] {
        crate::test_support::assert_code!(
            read(&started.reader, request).await,
            "unsupported_operation"
        );
    }
    crate::test_support::assert_code!(
        started
            .reader
            .request(
                &crate::runtime::hub::Caller::test_agent("agent"),
                api::RequestEnvelope::new(json!({"op":"ping"}))
            )
            .await,
        "unsupported_operation"
    );
    crate::test_support::assert_code!(
        read(&started.reader, json!({"op":"status", "target":"missing"})).await,
        "unknown_agent"
    );
    let ping = read(&started.reader, json!({"op":"ping"})).await.unwrap();
    assert_eq!(ping["writer_observation"]["source"], "committed");
    assert!(ping["writer_observation"]["sequence"].as_u64().unwrap() > 0);
    assert_eq!(
        ping["reader_observation"]["source"],
        "completed_reader_jobs"
    );
    assert_eq!(ping["reader_observation"]["counts"]["requests"], 1);
    assert_eq!(ping["reader_observation"]["counts"]["snapshots"], 1);
    finish(started);
    runtime.finish();
}
