//! Recovery regressions.
use super::*;

#[tokio::test]
async fn stalled_agent_reads_use_reader_while_writer_held_and_fence_in_snapshot() {
    let runtime = RuntimeFixture::new();
    seed_stalled_agent(&runtime).await;
    let started = start(&runtime).await;
    assert!(runtime.hub.reader.set(started.reader.downgrade()).is_ok());
    let baseline = runtime
        .hub
        .dispatch(agent_caller("turn"), "read".into(), stalled_request())
        .await
        .unwrap();
    assert_eq!(baseline["groups"][0]["root_id"], "quiet");
    let (entered, wait) = oneshot::channel();
    let (release, released) = WriterGate::new();
    let hub = runtime.hub.clone();
    let write = tokio::spawn(async move {
        hub.with(move |core| {
            let mut a = core.store().agent("agent")?;
            a.turn_id = Some("new".into());
            core.store().save(&a)?;
            core.store()
                .enqueue("new-mail", "human", "quiet", "queued")?;
            entered.send(()).unwrap();
            released.recv().unwrap();
            Ok(())
        })
        .await
    });
    wait.await.unwrap();
    let before = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        runtime
            .hub
            .dispatch(agent_caller("turn"), "read".into(), stalled_request()),
    )
    .await;
    release.release();
    write.await.unwrap().unwrap();
    assert_eq!(before.unwrap().unwrap()["groups"][0]["root_id"], "quiet");
    crate::test_support::assert_code!(
        runtime
            .hub
            .dispatch(agent_caller("turn"), "read".into(), stalled_request())
            .await,
        "stale_execution"
    );
    let after = runtime
        .hub
        .dispatch(agent_caller("new"), "read".into(), stalled_request())
        .await
        .unwrap();
    assert!(after["groups"].as_array().unwrap().is_empty());
    let mut wrong = agent_caller("new");
    if let super::super::super::hub::Caller::Agent { execution, .. } = &mut wrong {
        *execution = "other".into();
    }
    crate::test_support::assert_code!(
        runtime
            .hub
            .dispatch(wrong, "read".into(), stalled_request())
            .await,
        "stale_execution"
    );
    // A cloned Hub still has only a weak sender: reader joins before any Hub drops.
    let retained = runtime.hub.clone();
    finish(started);
    crate::test_support::assert_code!(
        retained
            .dispatch(agent_caller("new"), "read".into(), stalled_request())
            .await,
        "daemon_stopping"
    );
    drop(retained);
    runtime.finish();
}

#[tokio::test]
async fn snapshot_cleanup_failure_returns_primary_and_stops_reader_owner() {
    let runtime = RuntimeFixture::new();
    let queries = Queries::open_read_only(&runtime.temp.path().join("runtime.sqlite")).unwrap();
    queries
        .test_hooks()
        .at_sql("snapshot_close", |db| db.execute_batch("ROLLBACK"));
    let shutdown = runtime
        .hub
        .with(|c| Ok(c.shutdown().clone()))
        .await
        .unwrap();
    let started = Reader::start(
        queries,
        runtime.hub.published.clone(),
        shutdown.clone(),
        Arc::default(),
        runtime.hub.wake.clone(),
    );
    let result = read(&started.reader, json!({"op":"status", "target":"missing"})).await;
    assert_eq!(result.unwrap_err().code(), "unknown_agent");
    drop(started.reader);
    assert_eq!(
        started.thread.join().unwrap().unwrap_err().code(),
        "unknown_agent"
    );
    assert!(shutdown.load(Ordering::Relaxed));
    assert_eq!(runtime.hub.wake.failure().unwrap().code(), "unknown_agent");
    runtime.finish();
}
