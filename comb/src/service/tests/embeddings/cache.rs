use super::*;

#[test]
fn warm_cache_avoids_blob_reads_and_churn_preserves_old_snapshot_with_bounded_memory() {
    let (_dir, server, _) = setup(Some(Box::new(fake(Duration::ZERO))));
    let state = Arc::try_unwrap(server.state).ok().unwrap();
    let mut comb = state.writer.into_inner().unwrap();
    let mut engine = fake(Duration::ZERO);
    let (mutation, lease, cache) = warm_initial_page(&mut comb, &mut engine);
    comb.index = comb.index.with_vectors(Some(cache.clone()));
    let publication = comb.publication.clone();
    let path = comb.disk.index_path();
    let old = Snapshot::pin(
        Index::reader(&path, Some(cache.clone())).unwrap(),
        &publication,
    )
    .unwrap();
    let request = Search {
        query: "original".into(),
        ..Default::default()
    };
    old.index
        .retrieve(
            &request,
            Mode::Vector,
            embedding::run_query(&mut engine, lease.clone(), "original").into(),
        )
        .unwrap();
    assert_eq!(cache.misses.load(Ordering::Relaxed), 0);
    churn_page(&mut comb, &mut engine, mutation, &cache);
    let result = old
        .index
        .retrieve(
            &request,
            Mode::Vector,
            embedding::run_query(&mut engine, lease, "original").into(),
        )
        .unwrap();
    assert_eq!(result.results[0].snippet, "original");
    assert!(cache.len() <= 2);
    assert!(
        cache.misses.load(Ordering::Relaxed) > 0,
        "Evicted old snapshots safely fetch immutable cached data."
    );
    drop(comb);
    drop(old);
}

#[test]
fn text_only_server_does_not_warm_an_existing_embedding_space() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let mut comb = Comb::init(dir.path()).unwrap();
    let mut engine = fake(Duration::ZERO);
    comb.activate_embeddings(engine.space(), false).unwrap();
    comb.put_markdown(
        &"concepts/page".parse().unwrap(),
        "---\ntitle: Page\n---\nEvidence",
        None,
        &Write::new(&Actor::Human, &"create".parse().unwrap(), None).unwrap(),
    )
    .unwrap();
    let batch = comb
        .index()
        .unwrap()
        .embedding_jobs(&mut embedding::Scan::default(), 32)
        .unwrap();
    comb.apply_embeddings(embedding::run_batch(&mut engine, batch).unwrap())
        .unwrap();
    let server = Server::from_comb(comb, Options::default(), None)
        .unwrap()
        .bind()
        .unwrap();
    assert!(server.state.resident.is_none());
    assert!(
        !server
            .state
            .writer
            .lock()
            .unwrap()
            .index
            .has_resident_vectors()
    );
    assert!(
        server
            .state
            .snapshot()
            .unwrap()
            .index
            .document(&"concepts/page".parse().unwrap())
            .is_ok()
    );
}

fn churn_page(
    comb: &mut Comb,
    engine: &mut impl Embedder,
    mut mutation: crate::store::Mutation,
    cache: &crate::index::cache::Resident,
) {
    for i in 0..24 {
        mutation = comb
            .put_markdown(
                &"concepts/page".parse().unwrap(),
                &format!("---\ntitle: Page\n---\nrevision {i}"),
                Some(&mutation.hash),
                &Write::new(&Actor::Human, &format!("edit-{i}").parse().unwrap(), None).unwrap(),
            )
            .unwrap();
        let batch = comb
            .index()
            .unwrap()
            .embedding_jobs(&mut embedding::Scan::default(), 32)
            .unwrap();
        comb.apply_embeddings(embedding::run_batch(engine, batch).unwrap())
            .unwrap();
        assert!(cache.len() <= 2);
    }
}

fn warm_initial_page(
    comb: &mut Comb,
    engine: &mut impl Embedder,
) -> (
    crate::store::Mutation,
    embedding::Lease,
    Arc<crate::index::cache::Resident>,
) {
    let write = Write::new(&Actor::Human, &"initial".parse().unwrap(), None).unwrap();
    let mutation = comb
        .put_markdown(
            &"concepts/page".parse().unwrap(),
            "---\ntitle: Page\n---\noriginal",
            None,
            &write,
        )
        .unwrap();
    let batch = comb
        .index()
        .unwrap()
        .embedding_jobs(&mut embedding::Scan::default(), 32)
        .unwrap();
    comb.apply_embeddings(embedding::run_batch(engine, batch).unwrap())
        .unwrap();
    let lease = comb
        .index()
        .unwrap()
        .embedding_lease(engine.space())
        .unwrap();
    let cache = Arc::new(
        crate::index::cache::Resident::new(lease.clone(), 2 * (256 + 8), comb.health.clone())
            .unwrap(),
    );
    comb.index().unwrap().warm_vectors(&cache).unwrap();
    (mutation, lease, cache)
}
