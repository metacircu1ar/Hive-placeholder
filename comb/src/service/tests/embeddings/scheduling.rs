use super::*;
use crate::service::auth::ConnectionExt as _;
use std::fmt::Write as _;

#[test]
fn saturated_model_lane_preserves_reads_writes_and_actual_work_bounds() {
    let fake = fake(Duration::ZERO);
    let gate = fake.gate();
    let calls = fake.calls();
    let (_dir, server, human) = setup(Some(Box::new(fake)));
    let state = server.state.clone();
    runtime().block_on(async {
        let release = ModelRelease(gate);
        let running = Running::start(server);
        call(
            &human,
            &put("concepts/page", "Readable summary", "init"),
            None,
        )
        .await
        .unwrap();
        until(|| coverage(&state).embedded == 1).await;
        until(|| state.queries.slots.available_permits() == 64).await;
        let before = calls.load(Ordering::SeqCst);
        let a = human.clone();
        let first =
            tokio::spawn(async move { call(&a, &query("holdquery", Mode::Vector), None).await });
        until(|| calls.load(Ordering::SeqCst) > before).await;
        let mut waiting = Vec::new();
        for _ in 0..63 {
            let a = human.clone();
            waiting.push(tokio::spawn(async move {
                call(&a, &query("holdquery", Mode::Vector), None).await
            }));
        }
        until(|| state.queries.slots.available_permits() == 0).await;
        assert_text_and_write_progress(&human).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            before + 1,
            "One query holds the actual model; 63 jobs retain bounded queue slots."
        );
        drop(release);
        first.await.unwrap().unwrap();
        for query in waiting {
            if let Err(error) = query.await.unwrap() {
                assert_eq!(error.code, crate::ErrorCode::EmbeddingsBusy);
            }
        }
        until(|| state.queries.slots.available_permits() == 64).await;
        running.finish().await;
    });
}

#[test]
fn backfill_reserves_query_capacity_and_prioritizes_queries_between_batches() {
    let dir = crate::test_support::private_directory().unwrap();
    let mut comb = Comb::init(dir.path()).unwrap();
    let mut text = String::new();
    for i in 0..500 {
        write!(
            text,
            "## Section {i}\n{}\n\n",
            "needle evidence ".repeat(60)
        )
        .unwrap();
    }
    comb.put_markdown(
        &"concepts/backlog".parse().unwrap(),
        &format!("---\ntitle: Backlog\n---\n{text}"),
        None,
        &Write::new(&Actor::Human, &"create".parse().unwrap(), None).unwrap(),
    )
    .unwrap();
    let socket = dir.path().canonicalize().unwrap().join("comb.sock");
    let human = Connection::human(dir.path(), socket.clone()).unwrap();
    let server = Server::from_comb(
        comb,
        Options {
            socket: Some(socket),
            ..Default::default()
        },
        Some(Box::new(fake(Duration::from_millis(40)))),
    )
    .unwrap()
    .bind()
    .unwrap();
    let state = server.state.clone();
    runtime().block_on(async {
        let running = Running::start(server);
        until(|| coverage(&state).embedded > 0).await;
        let mut partial = false;
        for _ in 0..3 {
            let response = tokio::time::timeout(
                Duration::from_secs(2),
                call(&human, &query("needle", Mode::Hybrid), None),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(response["mode"], "hybrid");
            assert_eq!(response["fallback"], Value::Null);
            partial |= response["coverage"]["pending"].as_u64().unwrap() > 0;
        }
        assert!(
            partial,
            "Queries must work while document backfill remains in progress."
        );
        running.finish().await;
    });
}

#[test]
fn repeated_preemptions_reselect_only_the_pending_batch_without_the_writer() {
    let (_dir, server, _) = setup(Some(Box::new(fake(Duration::ZERO))));
    let state = server.state;
    let mut comb = state.writer.lock().unwrap();
    let mut engine = fake(Duration::ZERO);
    let mut text = String::new();
    for i in 0..2080 {
        write!(text, "## Section {i}\nEvidence number {i}\n\n").unwrap();
    }
    comb.put_markdown(
        &"concepts/corpus".parse().unwrap(),
        &format!("---\ntitle: Corpus\n---\n{text}"),
        None,
        &Write::new(&Actor::Human, &"create".parse().unwrap(), None).unwrap(),
    )
    .unwrap();
    let mut scan = embedding::Scan::default();
    for _ in 0..64 {
        let batch = comb.index().unwrap().embedding_jobs(&mut scan, 32).unwrap();
        comb.apply_embeddings(embedding::run_batch(&mut engine, batch).unwrap())
            .unwrap();
    }
    state
        .model
        .as_ref()
        .unwrap()
        .pending_queries
        .store(1, Ordering::SeqCst);
    // Keep the writer occupied: selection/preemption must use its own snapshot.
    for _ in 0..20 {
        let before = scan.steps;
        assert!(!state.backfill_batch(&mut scan).unwrap());
        assert!(
            scan.steps - before < 5000,
            "A 32-job retry must not rescan 2048 cached chunks."
        );
    }
    drop(comb);
    state
        .model
        .as_ref()
        .unwrap()
        .pending_queries
        .store(0, Ordering::SeqCst);
    assert!(state.backfill_batch(&mut scan).unwrap());
    assert!(!state.backfill_batch(&mut scan).unwrap());
}

async fn assert_text_and_write_progress(human: &Connection) {
    for _ in 0..40 {
        let found = tokio::time::timeout(
            Duration::from_secs(2),
            call(human, &query("summary", Mode::Hybrid), None),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(found["fallback"], "embeddings_busy");
        assert!(!found["results"].as_array().unwrap().is_empty());
    }
    assert_eq!(
        call(human, &query("summary", Mode::Vector), None)
            .await
            .unwrap_err()
            .code,
        crate::ErrorCode::EmbeddingsBusy
    );
    tokio::time::timeout(
        Duration::from_secs(2),
        call(
            human,
            &put("concepts/progress", "Writes continue", "progress"),
            None,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        call(
            human,
            &Operation::Get {
                slug: "concepts/progress".parse().unwrap()
            },
            None
        )
        .await
        .is_ok()
    );
}
