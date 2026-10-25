use super::*;
mod fixture;
use fixture::HttpFixture;

#[test]
fn http_queries_run_while_document_request_is_held() {
    let http = HttpFixture::new();
    let engine = Engine::load(http.config.clone()).unwrap();
    let (_dir, server, human) = setup(Some(Box::new(engine)));
    let state = server.state.clone();
    runtime().block_on(async {
        let running = Running::start(server);
        call(
            &human,
            &put("concepts/ready", "Searchable evidence", "ready"),
            None,
        )
        .await
        .unwrap();
        until(|| coverage(&state).embedded == 1).await;
        let held_document = http.hold_documents();
        call(&human, &put("concepts/slow", "Slow evidence", "slow"), None)
            .await
            .unwrap();
        until(|| http.documents_started() > 0).await;
        let response = call(&human, &query("evidence", Mode::Hybrid), None)
            .await
            .unwrap();
        assert!(http.document_waiting());
        assert_eq!(response["fallback"], Value::Null);
        assert_eq!(response["coverage"]["pending"], 1);
        assert_eq!(
            http.max_active.load(Ordering::SeqCst),
            2,
            "One document and one query request may run concurrently"
        );
        drop(held_document);
        until(|| http.active.load(Ordering::SeqCst) == 0).await;
        running.finish().await;
    });
}

#[test]
fn http_backfill_progresses_while_queries_wait_on_their_own_engine() {
    let http = HttpFixture::new();
    let engine = Engine::load(http.config.clone()).unwrap();
    let (_dir, server, human) = setup(Some(Box::new(engine)));
    let state = server.state.clone();
    runtime().block_on(async {
        let running = Running::start(server);
        call(
            &human,
            &put("concepts/ready", "Searchable evidence", "ready"),
            None,
        )
        .await
        .unwrap();
        until(|| coverage(&state).embedded == 1).await;
        let held_query = http.hold_queries();
        let first = human.clone();
        let held =
            tokio::spawn(async move { call(&first, &query("first", Mode::Vector), None).await });
        until(|| http.queries_started() == 1).await;
        let second = human.clone();
        let queued =
            tokio::spawn(async move { call(&second, &query("second", Mode::Vector), None).await });
        until(|| {
            state
                .model
                .as_ref()
                .unwrap()
                .pending_queries
                .load(Ordering::SeqCst)
                > 0
        })
        .await;
        call(&human, &put("concepts/new", "New evidence", "new"), None)
            .await
            .unwrap();
        until(|| coverage(&state).embedded == 2).await;
        assert!(
            http.query_waiting(),
            "Queued HTTP queries must not suppress document work"
        );
        assert_eq!(http.max_active.load(Ordering::SeqCst), 2);
        // Retain the real queued-query expiry while the first request stays held.
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), queued)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err()
                .code,
            crate::ErrorCode::EmbeddingsBusy
        );
        drop(held_query);
        held.await.unwrap().unwrap();
        running.finish().await;
    });
}

#[test]
fn http_input_rejection_does_not_block_later_backfill_batches() {
    let http = HttpFixture::new();
    let engine = Engine::load(http.config.clone()).unwrap();
    let (_dir, server, human) = setup(Some(Box::new(engine)));
    let text = rejected_corpus();
    runtime().block_on(async {
        let state = server.state.clone();
        let running = Running::start(server);
        call(&human, &put("concepts/corpus", &text, "create"), None)
            .await
            .unwrap();
        until(|| coverage(&state).pending == 0).await;
        let report = call(&human, &Operation::Doctor, None).await.unwrap();
        assert_eq!(report["embeddings"]["embedded"], 69);
        assert_eq!(report["embeddings"]["rejected"], 1);
        assert!(report["findings"].as_array().unwrap().iter().any(|f| {
            f["kind"] == "embedding_rejected"
                && f["detail"]
                    .as_str()
                    .unwrap()
                    .contains("embedding_input_rejected")
        }));
        assert_eq!(report["embedding_worker_error"], Value::Null);
        running.finish().await;
    });
}

#[test]
fn http_auth_and_outage_errors_remain_retryable_and_visible_in_doctor() {
    for status in [400, 401, 503] {
        let http = HttpFixture::new();
        http.status.store(status, Ordering::SeqCst);
        let engine = Engine::load(http.config.clone()).unwrap();
        let (_dir, server, human) = setup(Some(Box::new(engine)));
        runtime().block_on(async {
            let state = server.state.clone();
            let running = Running::start(server);
            call(
                &human,
                &put("concepts/page", "Still pending", "create"),
                None,
            )
            .await
            .unwrap();
            until(|| {
                state
                    .writer
                    .lock()
                    .unwrap()
                    .embedding_worker_error
                    .is_some()
            })
            .await;
            let report = call(&human, &Operation::Doctor, None).await.unwrap();
            assert_eq!(report["embeddings"]["pending"], 1);
            assert_eq!(report["embeddings"]["rejected"], 0);
            if status == 400 {
                assert_eq!(
                    report["embedding_worker_error"]["code"],
                    "embedding_endpoint_rejected"
                );
                assert_eq!(
                    call(&human, &query("valid", Mode::Vector), None)
                        .await
                        .unwrap_err()
                        .code,
                    crate::ErrorCode::EmbeddingEndpointRejected
                );
            } else {
                assert!(
                    report["embedding_worker_error"]["message"]
                        .as_str()
                        .unwrap()
                        .contains(&status.to_string())
                );
            }
            running.finish().await;
        });
    }
}

fn rejected_corpus() -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    for i in 0..70 {
        write!(
            text,
            "## Chunk {i}\n{}\n\n",
            if i == 3 {
                "REJECTME".into()
            } else {
                format!("Valid evidence {i}")
            }
        )
        .unwrap();
    }
    text
}
