use super::*;

#[test]
fn writes_cannot_bypass_transient_embedding_backoff() {
    let engine = fake(Duration::ZERO).batch_error(Error::new(
        crate::ErrorCode::FixtureUnavailable,
        "Transient fixture failure",
    ));
    let calls = engine.calls();
    let (_dir, server, human) = setup(Some(Box::new(engine)));
    runtime().block_on(async {
        let running = Running::start(server);
        call(&human, &put("concepts/initial", "First", "initial"), None)
            .await
            .unwrap();
        until(|| calls.load(Ordering::SeqCst) == 1).await;
        let start = std::time::Instant::now();
        let mut n = 0;
        while start.elapsed() < Duration::from_millis(600) {
            n += 1;
            call(
                &human,
                &put(&format!("concepts/page-{n}"), "More", &format!("write-{n}")),
                None,
            )
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(n > 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        until(|| calls.load(Ordering::SeqCst) == 2).await;
        assert!(start.elapsed() >= Duration::from_secs(4));
        running.finish().await;
    });
}
