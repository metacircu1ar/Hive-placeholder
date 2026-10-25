//! Actual elapsed-time contracts remain separate from progress handshakes.
//!
//! | Contract | Bound |
//! | --- | --- |
//! | Queued query | 1 s admission expiry; caller <2 s, 62 then 64 permits |
//! | Transient backoff | 5 s retry; repeated writes for 600 ms cannot bypass it; retry >=4 s |
//! | Adaptive work | 40 ms/document input, 2 ms/query input; 64+64 within 5376 ms |
//!
//! HTTP body expiry remains in `embedding::tests::http_limits` (1 s budget,
//! <4 s completion; 3 s read and 5 s release watchdogs). Submitted administration
//! retains its 50 ms injected deadline in `service::tests::lifecycle`.
use super::*;
mod adaptive;
mod backoff;

#[test]
fn queued_query_deadline_discards_work_without_releasing_inflight_capacity_early() {
    let engine = fake(Duration::ZERO);
    let gate = engine.gate();
    let calls = engine.calls();
    let (_dir, server, human) = setup(Some(Box::new(engine)));
    let state = server.state.clone();
    runtime().block_on(async {
        let release = ModelRelease(gate);
        let running = Running::start(server);
        call(&human, &put("concepts/page", "Evidence", "create"), None)
            .await
            .unwrap();
        until(|| coverage(&state).embedded == 1).await;
        let before = calls.load(Ordering::SeqCst);
        let client = human.clone();
        let held =
            tokio::spawn(
                async move { call(&client, &query("holdquery", Mode::Vector), None).await },
            );
        until(|| calls.load(Ordering::SeqCst) > before).await;
        let deadline = std::time::Instant::now();
        assert_eq!(
            call(&human, &query("should never infer", Mode::Vector), None)
                .await
                .unwrap_err()
                .code,
            crate::ErrorCode::EmbeddingsBusy
        );
        assert!(deadline.elapsed() < Duration::from_secs(2));
        assert_eq!(state.queries.slots.available_permits(), 62);
        drop(release);
        held.await.unwrap().unwrap();
        until(|| state.queries.slots.available_permits() == 64).await;
        assert_eq!(calls.load(Ordering::SeqCst), before + 1);
        running.finish().await;
    });
}
