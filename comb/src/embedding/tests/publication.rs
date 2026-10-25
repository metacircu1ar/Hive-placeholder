use super::*;
use crate::{
    ErrorCode,
    index::{Index, cache::Resident},
    read::Snapshot,
};
use rusqlite::{OptionalExtension, params};
use std::sync::{Arc, atomic::Ordering};

struct Fixture {
    comb: Comb,
    batch: Batch,
    completed: Completed,
    cache: Arc<Resident>,
    _root: tempfile::TempDir,
}

fn two_jobs() -> Fixture {
    let (root, mut comb, mut mock) = setup();
    put(&mut comb, "concepts/cat", "Cat", "feline");
    put(&mut comb, "concepts/dog", "Dog", "canine");
    let batch = comb
        .index()
        .unwrap()
        .embedding_jobs(&mut Scan::default(), 2)
        .unwrap();
    assert_eq!(batch.jobs.len(), 2);
    assert_ne!(batch.jobs[0].input_hash, batch.jobs[1].input_hash);
    let completed = run_batch(&mut mock, batch.clone()).unwrap();
    let cache = Arc::new(Resident::new(batch.lease.clone(), 1024, comb.health.clone()).unwrap());
    comb.index = comb.index.with_vectors(Some(cache.clone()));
    Fixture {
        _root: root,
        comb,
        batch,
        completed,
        cache,
    }
}

fn readable_snapshot(comb: &Comb) -> Snapshot {
    comb.health.check().unwrap();
    comb.ready().unwrap();
    assert_eq!(comb.index().unwrap().stats().unwrap().pages, 2);
    let snapshot = Snapshot::pin(
        Index::reader(&comb.disk.index_path(), None).unwrap(),
        &comb.publication,
    )
    .unwrap();
    for (slug, text) in [("concepts/cat", "feline"), ("concepts/dog", "canine")] {
        let slug = slug.parse().unwrap();
        assert_eq!(comb.get(&slug).unwrap().page.compiled, text);
        assert_eq!(snapshot.index.document(&slug).unwrap().page.compiled, text);
    }
    snapshot
}

fn stored_vector(snapshot: &Snapshot, lease: &Lease, hash: &ContentHash) -> Option<Vec<u8>> {
    snapshot
        .index
        .test_connection()
        .query_row(
            "SELECT vector FROM vector_cache.vectors WHERE fingerprint=?1 AND input_hash=?2",
            params![lease.fingerprint, hash],
            |row| row.get(0),
        )
        .optional()
        .unwrap()
}

#[test]
fn ordinary_cache_error_after_commit_preserves_readiness_and_committed_rows() {
    let mut fixture = two_jobs();
    let lease = &fixture.batch.lease;
    let first = &fixture.batch.jobs[0].input_hash;
    let second = &fixture.batch.jobs[1].input_hash;
    fixture
        .comb
        .index
        .insert_malformed_vector(&lease.fingerprint, first)
        .unwrap();
    let before = readable_snapshot(&fixture.comb);
    assert!(stored_vector(&before, lease, second).is_none());
    drop(before);

    let error = fixture
        .comb
        .apply_embeddings(fixture.completed)
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidEmbedding);
    assert!(!error.file_committed);
    assert_eq!(fixture.cache.len(), 0);
    assert_eq!(fixture.cache.misses.load(Ordering::Relaxed), 1);
    let after = readable_snapshot(&fixture.comb);
    assert_eq!(stored_vector(&after, lease, first).unwrap(), vec![0_u8; 8]);
    // The first immutable row predates apply; the second proves an actual COMMIT
    // visible to a new connection before resident publication failed.
    let committed = stored_vector(&after, lease, second).unwrap();
    assert!(decode(&committed, lease.dimensions).is_ok());
}

#[test]
fn derived_transaction_rollback_preserves_readiness_without_publishing_cache_entries() {
    let mut fixture = two_jobs();
    let lease = &fixture.batch.lease;
    let second = &fixture.batch.jobs[1].input_hash;
    fixture
        .comb
        .index
        .reject_second_vector(&lease.fingerprint, second)
        .unwrap();
    let error = fixture
        .comb
        .apply_embeddings(fixture.completed)
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::IndexError);
    assert!(!error.file_committed);
    assert!(fixture.comb.index.test_connection().is_autocommit());
    assert_eq!(fixture.cache.len(), 0);
    assert_eq!(fixture.cache.misses.load(Ordering::Relaxed), 0);
    let after = readable_snapshot(&fixture.comb);
    for job in &fixture.batch.jobs {
        assert!(stored_vector(&after, lease, &job.input_hash).is_none());
    }
    drop(after);
    fixture.comb.index.allow_second_vector().unwrap();
}
