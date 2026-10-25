use super::*;

#[test]
fn sixty_four_socket_searches_coalesce_while_backfill_is_active() {
    let engine = fake(Duration::from_millis(40)).hold_on("q:");
    let gate = engine.gate();
    let query_calls = engine.query_calls();
    let (_dir, server, human) = setup(Some(Box::new(engine)));
    let state = server.state.clone();
    runtime().block_on(async {
        let release = ModelRelease(gate);
        let running = Running::start(server);
        let text = needle_corpus(500);
        call(&human, &put("concepts/corpus", &text, "create"), None)
            .await
            .unwrap();
        until(|| coverage(&state).embedded > 0).await;
        let searches = simultaneous_searches(&human);
        until(|| state.queries.slots.available_permits() == 0).await;
        drop(release);
        for search in searches {
            let response = search.await.unwrap().unwrap();
            assert_eq!(response["fallback"], Value::Null);
            assert_eq!(response["mode"], "hybrid");
            assert!(response["coverage"]["embedded"].as_u64().unwrap() > 0);
        }
        let calls = query_calls.load(Ordering::SeqCst);
        assert!(
            (2..=6).contains(&calls),
            "64 queries should coalesce into a few calls, got {calls}"
        );
        running.finish().await;
    });
}
