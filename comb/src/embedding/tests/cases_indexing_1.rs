//! Indexing regressions.
use super::*;

#[test]
fn cache_poison_during_publication_fences_without_relocking_the_gate() {
    let (done, received) = mpsc::channel();
    let worker = thread::spawn(move || {
        let (_root, mut comb, mut mock) = setup();
        put(&mut comb, "concepts/cat", "Cat", "feline");
        let batch = comb
            .index()
            .unwrap()
            .embedding_jobs(&mut Scan::default(), 32)
            .unwrap();
        let cache = std::sync::Arc::new(
            crate::index::cache::Resident::new(batch.lease.clone(), 1024, comb.health.clone())
                .unwrap(),
        );
        cache.poison_on_next_read();
        comb.index = comb.index.with_vectors(Some(cache));
        let completed = run_batch(&mut mock, batch).unwrap();
        let error = comb.apply_embeddings(completed).unwrap_err();
        assert_eq!(error.code, crate::ErrorCode::RecoveryRequired);
        assert_eq!(
            comb.ready().unwrap_err().code,
            crate::ErrorCode::RecoveryRequired
        );
        // The derived transaction committed before resident publication failed.
        assert_eq!(
            comb.index
                .embedding_coverage(crate::search::Filter::default())
                .unwrap()
                .embedded,
            1
        );
        done.send(()).unwrap();
    });
    received
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    worker.join().unwrap();
}

#[test]
fn delayed_jobs_are_fenced_after_edit_delete_recreate_and_model_aba() {
    let (_root, mut comb, mut mock) = setup();
    put(&mut comb, "concepts/cat", "Cat", "feline");
    let old = complete_pending(&comb, &mut mock);
    put(&mut comb, "concepts/cat", "Cat", "canine");
    assert_eq!(comb.apply_embeddings(old).unwrap().stale, 1);
    let old = complete_pending(&comb, &mut mock);
    let hash = comb.get(&"concepts/cat".parse().unwrap()).unwrap().hash;
    comb.remove(
        &"concepts/cat".parse().unwrap(),
        &hash,
        &Write::new(&Actor::Human, &"remove-cat".parse().unwrap(), None).unwrap(),
    )
    .unwrap();
    put(&mut comb, "concepts/cat", "Cat", "canine");
    assert_eq!(comb.apply_embeddings(old).unwrap().stale, 1);
    let old = complete_pending(&comb, &mut mock);
    let other = mock_embedder("b");
    assert_eq!(
        comb.activate_embeddings(other.space(), false)
            .unwrap_err()
            .code,
        crate::ErrorCode::EmbeddingReplaceRequired
    );
    comb.activate_embeddings(other.space(), true).unwrap();
    comb.activate_embeddings(mock.space(), true).unwrap();
    assert_eq!(comb.apply_embeddings(old).unwrap().stale, 1);
    assert_eq!(coverage(&comb).embedded, 0);
}

#[test]
fn rebuild_reuses_vectors_and_only_changed_inputs_need_inference() {
    let (root, mut comb, mut mock) = setup();
    put(&mut comb, "concepts/cat", "Cat", "feline");
    put(&mut comb, "concepts/dog", "Dog", "canine");
    fill(&mut comb, &mut mock);
    let q = query(&comb, &mut mock, "kitten");
    let before = comb
        .index()
        .unwrap()
        .retrieve(&request("kitten"), Mode::Vector, QueryVector::Vector(q))
        .unwrap();
    drop(comb);
    let mut comb = Comb::rebuild(root.path()).unwrap();
    comb.activate_embeddings(mock.space(), false).unwrap();
    let calls = mock.input_count();
    fill(&mut comb, &mut mock);
    assert_eq!(mock.input_count(), calls);
    let q = query(&comb, &mut mock, "kitten");
    let after = comb
        .index()
        .unwrap()
        .retrieve(&request("kitten"), Mode::Vector, QueryVector::Vector(q))
        .unwrap();
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(after).unwrap()
    );
    put(&mut comb, "concepts/dog", "Dog", "different canine input");
    let calls = mock.input_count();
    fill(&mut comb, &mut mock);
    assert_eq!(mock.input_count(), calls + 1);
}

#[test]
fn vector_filters_precede_ranking_and_pages_collapse_before_fusion() {
    let (_root, mut comb, mut mock) = setup();
    put(
        &mut comb,
        "concepts/alpha",
        "Alpha",
        "feline\n\n## More\nfeline again\n\n## Third\nfeline third",
    );
    put(&mut comb, "concepts/beta", "Beta", "feline");
    put(&mut comb, "reports/gamma", "Gamma", "feline");
    fill(&mut comb, &mut mock);
    let mut req = request("kitten");
    req.prefix = Some("concepts/".into());
    req.limit = 2;
    let q = query(&comb, &mut mock, &req.query);
    let result = comb
        .index()
        .unwrap()
        .retrieve(&req, Mode::Vector, QueryVector::Vector(q))
        .unwrap();
    assert_eq!(
        result
            .results
            .iter()
            .map(|h| h.slug.as_str())
            .collect::<Vec<_>>(),
        ["concepts/alpha", "concepts/beta"]
    );
    assert!((result.results[0].score - 1.0 / 61.0).abs() < 1e-12);
    assert!((result.results[1].score - 1.0 / 62.0).abs() < 1e-12);
    assert_eq!(result.coverage.unwrap().total, 4);
    req.query = "feline".into();
    let q = query(&comb, &mut mock, &req.query);
    let result = comb
        .index()
        .unwrap()
        .retrieve(&req, Mode::Hybrid, QueryVector::Vector(q))
        .unwrap();
    // Independently known equal-vector ranks and one text rank per page.
    assert_eq!(result.results.len(), 2);
    assert!(
        result
            .results
            .iter()
            .all(|h| h.matched_by == Route::TextVector)
    );
    assert!(
        (result.results.iter().map(|h| h.score).sum::<f64>() - 2.0 / 61.0 - 2.0 / 62.0).abs()
            < 1e-12
    );
}

#[test]
fn invalid_vectors_and_corrupt_cache_never_reach_search() {
    for vector in [
        vec![0.0, 0.0],
        vec![f32::NAN, 1.0],
        vec![f32::INFINITY, 0.0],
        vec![1.0],
    ] {
        assert!(normalize(vector, 2).is_err());
    }
    let (_root, mut comb, mut mock) = setup();
    put(&mut comb, "concepts/cat", "Cat", "feline");
    let batch = comb
        .index()
        .unwrap()
        .embedding_jobs(&mut Scan::default(), 32)
        .unwrap();
    assert!(
        comb.apply_embeddings(Completed {
            batch,
            vectors: vec![Ok(vec![0.0, 0.0])]
        })
        .is_err()
    );
    fill(&mut comb, &mut mock);
    comb.index.corrupt_cached_vectors().unwrap();
    let q = query(&comb, &mut mock, "kitten");
    assert_eq!(
        comb.index()
            .unwrap()
            .retrieve(&request("kitten"), Mode::Vector, QueryVector::Vector(q))
            .err()
            .unwrap()
            .code,
        crate::ErrorCode::InvalidEmbedding
    );
    let q = query(&comb, &mut mock, "kitten");
    assert_eq!(
        comb.index()
            .unwrap()
            .retrieve(&request("kitten"), Mode::Hybrid, QueryVector::Vector(q))
            .unwrap()
            .fallback,
        Some(crate::ErrorCode::InvalidEmbedding)
    );
}

#[test]
fn cursor_wrap_preserves_edits_and_model_switch() {
    let (_root, mut comb, mut mock) = setup();
    put(
        &mut comb,
        "concepts/corpus",
        "Corpus",
        &many_chunks(40, "old"),
    );
    let mut scan = Scan::default();
    let held = run_batch(
        &mut mock,
        comb.index().unwrap().embedding_jobs(&mut scan, 16).unwrap(),
    )
    .unwrap();
    put(
        &mut comb,
        "concepts/corpus",
        "Corpus",
        &many_chunks(40, "new"),
    );
    assert_eq!(comb.apply_embeddings(held).unwrap().stale, 16);
    // The changed first 16 rows may be behind the cursor; finish the pass, then
    // wrap and find them. A service retries a pass after mutation notification.
    for _ in 0..2 {
        loop {
            let batch = comb.index().unwrap().embedding_jobs(&mut scan, 16).unwrap();
            if batch.jobs.is_empty() {
                break;
            }
            comb.apply_embeddings(run_batch(&mut mock, batch).unwrap())
                .unwrap();
        }
    }
    assert_eq!(coverage(&comb).embedded, 40);
    let other = mock_embedder("other");
    comb.activate_embeddings(other.space(), true).unwrap();
    assert_eq!(
        comb.index()
            .unwrap()
            .embedding_jobs(&mut scan, 16)
            .unwrap()
            .jobs
            .len(),
        16
    );
}

#[test]
fn only_final_hits_load_chunk_text_in_all_search_modes() {
    let (_root, mut comb, mut mock) = setup();
    for n in 0..12 {
        put(
            &mut comb,
            &format!("concepts/page-{n}"),
            "Large body",
            &format!("feline {}", "x".repeat(60_000)),
        );
    }
    fill(&mut comb, &mut mock);
    for mode in [Mode::Text, Mode::Vector, Mode::Hybrid] {
        crate::index::search::HYDRATED.set(0);
        let mut req = request("feline");
        req.limit = 1;
        let q = query(&comb, &mut mock, &req.query);
        let results = comb
            .index()
            .unwrap()
            .retrieve(&req, mode, QueryVector::Vector(q))
            .unwrap();
        assert_eq!(results.results.len(), 1);
        assert_eq!(crate::index::search::HYDRATED.get(), 1);
        assert!(results.results[0].snippet.chars().count() <= 480);
    }
}

#[test]
fn doctor_probes_embedding_failure_overflow_beyond_the_finding_limit() {
    let (_root, mut comb, mock) = setup();
    put(&mut comb, "concepts/failures", "Failures", "# Failures");
    let lease = comb.index().unwrap().embedding_lease(mock.space()).unwrap();
    // Populate only derived state to avoid 1025 unrelated durable writes.
    comb.index
        .seed_embedding_failures(&lease.fingerprint)
        .unwrap();
    assert_eq!(
        comb.index().unwrap().embedding_findings().unwrap().len(),
        1025
    );
    let report = comb.doctor().unwrap();
    assert_eq!(report.findings.len(), 1024);
    assert!(report.findings_truncated);
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.kind == FindingKind::EmbeddingRejected)
    );
    assert_eq!(report.embeddings.rejected, 1025);
}
