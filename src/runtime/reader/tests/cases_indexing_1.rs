//! Indexing regressions.
use super::*;

#[tokio::test]
async fn rolled_back_readiness_candidate_is_not_cached_or_published() {
    let runtime = RuntimeFixture::new();
    runtime
        .hub
        .with(|core| {
            let mut agent = core.store().agent("agent")?;
            agent.state = crate::runtime::domain::AgentState::Idle;
            core.store().save(&agent)
        })
        .await
        .unwrap();
    let started = start(&runtime).await;
    assert_eq!(
        read(&started.reader, json!({"op":"ping"})).await.unwrap()["storage"]["ready"],
        true
    );
    let publication = runtime.hub.published.clone();
    let failed: Result<()> = runtime
        .hub
        .with(move |core| {
            let mut agent = core.store().agent("agent")?;
            agent.paused = true;
            core.store().save(&agent)?;
            assert!(!publication.prepare(core.store())?.ready);
            // A provider/batch read still observes its own uncommitted transaction.
            assert!(!core.store().has_ready()?);
            Err(Error::invalid_request("roll back readiness mutation"))
        })
        .await;
    crate::test_support::assert_code!(failed, "invalid_request");
    assert_eq!(
        read(&started.reader, json!({"op":"ping"})).await.unwrap()["storage"]["ready"],
        true
    );
    // total_changes no longer advances; the read-only batch must not reuse the
    // discarded false candidate from before rollback.
    runtime
        .hub
        .with(|core| {
            assert!(core.store().has_ready()?);
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        read(&started.reader, json!({"op":"ping"})).await.unwrap()["storage"]["ready"],
        true
    );
    finish(started);
    runtime.finish();
}
