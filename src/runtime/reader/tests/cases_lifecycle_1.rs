//! Lifecycle regressions.
use super::*;

#[tokio::test]
async fn purge_barrier_preserves_an_output_read_paused_between_lookup_and_open() {
    let runtime = RuntimeFixture::new();
    let artifact = output_fixture(&runtime).await;
    let queries = Queries::open_read_only(&runtime.temp.path().join("runtime.sqlite")).unwrap();
    let (entered, wait) = oneshot::channel();
    let (release, released) = WriterGate::new();
    queries.test_hooks().before_output(move || {
        entered.send(()).unwrap();
        released.recv().unwrap();
    });
    let shutdown = runtime
        .hub
        .with(|c| Ok(c.shutdown().clone()))
        .await
        .unwrap();
    let started = Reader::start(
        queries,
        runtime.hub.published.clone(),
        shutdown,
        Arc::default(),
        runtime.hub.wake.clone(),
    );
    let reader = started.reader.clone();
    let id = artifact.clone();
    let reading = tokio::spawn(async move { read(&reader, json!({"op":"output","id":id})).await });
    if !matches!(
        tokio::time::timeout(std::time::Duration::from_secs(2), wait).await,
        Ok(Ok(()))
    ) {
        release.release();
        reading.abort();
        let _ = reading.await;
        finish(started);
        runtime.finish();
        panic!("artifact read did not reach its pre-open hook");
    }
    runtime
        .hub
        .with(|c| {
            c.store().purge("agent", false, false, c.running())?;
            Ok(())
        })
        .await
        .unwrap();
    let reader = started.reader.clone();
    let mut barrier = tokio::spawn(async move { reader.barrier().await });
    let blocked = tokio::time::timeout(std::time::Duration::from_millis(50), &mut barrier)
        .await
        .is_err();
    let ping_start = Instant::now();
    let ping = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        read(&started.reader, json!({"op":"ping"})),
    )
    .await;
    let ping_elapsed = ping_start.elapsed();
    release.release();
    let output = reading.await.unwrap().unwrap();
    barrier.await.unwrap().unwrap();
    assert!(blocked);
    assert!(output["text"].as_str().unwrap().contains('x'));
    unlink_purged_artifacts(&runtime).await;
    assert_eq!(
        read(&started.reader, json!({"op":"output","id":artifact}))
            .await
            .unwrap_err()
            .code(),
        "unknown_artifact"
    );
    finish(started);
    runtime.finish();
    ping.expect("health Ping waited behind an artifact read")
        .unwrap();
    eprintln!("Ping during blocked artifact read: {ping_elapsed:?}");
}
