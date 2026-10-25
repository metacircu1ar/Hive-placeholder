use crate::{ErrorCode, Index, disk::golden::seeded_comb, embedding::Scan, store::List};
use rusqlite::Connection;

fn empty_index() -> Index {
    Index {
        connection: Connection::open_in_memory().unwrap(),
        vectors: None,
    }
}

#[test]
fn list_and_link_bounds_precede_database_access() {
    let index = empty_index();
    let slug = "concepts/root".parse().unwrap();
    for (limit, expected) in [
        (0, ErrorCode::InvalidLimit),
        (1, ErrorCode::IndexError),
        (1000, ErrorCode::IndexError),
        (1001, ErrorCode::InvalidLimit),
        (usize::MAX, ErrorCode::InvalidLimit),
    ] {
        let request = List {
            limit: Some(limit),
            ..List::default()
        };
        assert_eq!(index.list(&request).unwrap_err().code, expected);
        for incoming in [false, true] {
            assert_eq!(
                index
                    .links_page(&slug, incoming, None, limit)
                    .unwrap_err()
                    .code,
                expected
            );
        }
    }
}

#[test]
fn graph_depth_and_batch_bounds_precede_database_access() {
    let index = empty_index();
    let slug = "concepts/root".parse().unwrap();
    for (depth, expected) in [
        (0, ErrorCode::IndexError),
        (8, ErrorCode::IndexError),
        (9, ErrorCode::InvalidDepth),
    ] {
        assert_eq!(index.graph(&slug, depth).unwrap_err().code, expected);
    }
    for (limit, expected) in [
        (0, ErrorCode::InvalidEmbeddingBatch),
        (1, ErrorCode::IndexError),
        (32, ErrorCode::IndexError),
        (33, ErrorCode::InvalidEmbeddingBatch),
        (usize::MAX, ErrorCode::InvalidEmbeddingBatch),
    ] {
        assert_eq!(
            index
                .embedding_jobs(&mut Scan::default(), limit)
                .err()
                .unwrap()
                .code,
            expected
        );
    }
}

#[test]
fn degree_sentinel_rejects_the_next_neighbor_before_decoding_links() {
    let page = crate::test_support::PageBuilder::new("Root", "Root.")
        .build()
        .unwrap();
    let (_root, comb) = seeded_comb(&[("concepts/root", page)]);
    let index = comb.index().unwrap();
    index.seed_invalid_graph_neighbors().unwrap();
    let slug = "concepts/root".parse().unwrap();
    assert_eq!(
        index.graph(&slug, 1).unwrap_err().code,
        ErrorCode::IndexError
    );
    index.add_graph_degree_sentinel().unwrap();
    assert_eq!(
        index.graph(&slug, 1).unwrap_err().code,
        ErrorCode::GraphLimit
    );
}
