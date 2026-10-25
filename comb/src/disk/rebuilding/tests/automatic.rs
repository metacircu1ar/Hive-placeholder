use super::*;
use crate::{
    embedding::{self, Scan, Space},
    search::Filter,
    test_support::embedding::FakeEmbedder,
};

fn embedded() -> (tempfile::TempDir, Space) {
    let root = fixture(false);
    let mut comb = Comb::open(root.path()).unwrap();
    let space = Space {
        backend: "mock".into(),
        model: "synthetic".into(),
        revision: "fixed".into(),
        dimensions: 2,
        document_prefix: String::new(),
        query_prefix: String::new(),
        preprocessing: "fixed".into(),
        normalization: "l2".into(),
    };
    comb.activate_embeddings(&space, false).unwrap();
    let batch = comb.index.embedding_jobs(&mut Scan::default(), 32).unwrap();
    assert!(!batch.jobs.is_empty());
    let mut engine = FakeEmbedder::new(space.clone(), |_| Ok(vec![1.0, 0.0]));
    comb.apply_embeddings(embedding::run_batch(&mut engine, batch).unwrap())
        .unwrap();
    comb.index.add_unexpected_table().unwrap();
    drop(comb);
    (root, space)
}

#[test]
fn interrupted_automatic_rebuild_preserves_embedding_selection_and_vectors() {
    let (root, space) = embedded();
    let path = root.path().join(".comb/vectors.sqlite");
    let original = fs::read(&path).unwrap();
    let error = hooks::with(
        |path, step| {
            if path.file_name().unwrap() == POPULATION && step == Step::AfterPublish {
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
            Ok(())
        },
        || Comb::open(root.path()),
    )
    .err()
    .unwrap();
    assert!(error.file_committed, "{error:?}");
    let comb = Comb::open(root.path()).unwrap();
    assert_eq!(comb.index.active_space().unwrap().unwrap().1, space);
    assert_eq!(
        comb.index
            .embedding_coverage(Filter::default())
            .unwrap()
            .pending,
        0
    );
    assert!(
        !comb
            .get(&"concepts/kept".parse().unwrap())
            .unwrap()
            .unverified
    );
    drop(comb);
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn automatic_text_rebuild_never_authorizes_discarding_unknown_vector_format() {
    let (root, _) = embedded();
    let path = root.path().join(".comb/vectors.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE future(value TEXT)").unwrap();
    drop(db);
    let original = fs::read(&path).unwrap();
    assert!(Comb::open(root.path()).is_err());
    assert_eq!(fs::read(path).unwrap(), original);
    assert!(!root.path().join(".comb/index-rebuild.json").exists());
}
