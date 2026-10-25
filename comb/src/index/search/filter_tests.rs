use super::{bind, sql};
use crate::{
    ErrorCode, Page,
    disk::golden::seeded_comb,
    embedding::Space,
    search::{Filter, Search},
    values::{CreateSafety, MatchEvidence},
};
use rusqlite::{Connection, ToSql};
use std::collections::BTreeSet;

#[test]
fn named_bindings_are_complete_order_independent_and_reset_nulls() {
    let db = Connection::open_in_memory().unwrap();
    let mut statement = db
        .prepare("SELECT :query,:before,:kind,:lower,:prefix,:after,:tag")
        .unwrap();
    let after = "2026-01-02T03:04:05+02:00".parse().unwrap();
    let before = "2026-02-03T04:05:06Z".parse().unwrap();
    let filter = Filter {
        kind: Some("concept"),
        tag: Some("evidence"),
        prefix: Some("concepts/"),
        after: Some(&after),
        before: Some(&before),
    };
    let extra: [(&str, &dyn ToSql); 2] = [(":lower", &"raw lower"), (":query", &"escaped query")];
    bind(&mut statement, filter, &extra).unwrap();
    let values: Vec<String> = statement
        .raw_query()
        .mapped(|row| (0..7).map(|i| row.get(i)).collect())
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(
        values,
        [
            "escaped query",
            "2026-02-03T04:05:06Z",
            "concept",
            "raw lower",
            "concepts/",
            "2026-01-02T03:04:05+02:00",
            "evidence",
        ]
    );
    bind(&mut statement, Filter::default(), &extra).unwrap();
    let values: Vec<Option<String>> = statement
        .raw_query()
        .mapped(|row| (0..7).map(|i| row.get(i)).collect())
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(
        values,
        [
            Some("escaped query".into()),
            None,
            None,
            Some("raw lower".into()),
            None,
            None,
            None,
        ]
    );
}

#[test]
fn named_bindings_reject_missing_unknown_duplicate_and_positional_parameters() {
    let db = Connection::open_in_memory().unwrap();
    let mut statement = db
        .prepare("SELECT :query,:lower,:kind,:tag,:prefix,:after,:before")
        .unwrap();
    let cases: [&[(&str, &dyn ToSql)]; 5] = [
        &[(":query", &"text")],
        &[(":query", &"text"), (":typo", &"lower")],
        &[(":query", &"text"), (":query", &"lower")],
        &[(":query", &"text"), (":kind", &"lower")],
        &[
            (":query", &"text"),
            (":lower", &"lower"),
            (":unused", &"unused"),
        ],
    ];
    for extra in cases {
        assert!(matches!(
            bind(&mut statement, Filter::default(), extra),
            Err(rusqlite::Error::InvalidParameterCount(..)
                | rusqlite::Error::InvalidParameterName(_))
        ));
    }
    let mut positional = db
        .prepare("SELECT ?1,:kind,:tag,:prefix,:after,:before")
        .unwrap();
    assert!(matches!(
        bind(&mut positional, Filter::default(), &[(":query", &"text")]),
        Err(rusqlite::Error::InvalidParameterName(_))
    ));
}

#[test]
fn every_production_query_has_exact_filter_and_route_parameters() {
    let (_root, comb) = seeded_comb(&[]);
    let index = comb.index().unwrap();
    for query in [
        sql::FTS,
        sql::IDENTITY_SHORT,
        sql::IDENTITY_LONG,
        sql::EXACT,
        sql::VECTOR_HASH,
        sql::VECTOR_BLOB,
        sql::COVERAGE,
    ] {
        let mut statement = index.connection.prepare(query).unwrap();
        let mut expected = BTreeSet::from([":kind", ":tag", ":prefix", ":after", ":before"]);
        let extra: Vec<(&str, &dyn ToSql)> = if query == sql::COVERAGE {
            vec![(":fingerprint", &"fingerprint")]
        } else if query == sql::VECTOR_HASH || query == sql::VECTOR_BLOB {
            vec![(":lower", &"needle"), (":fingerprint", &"fingerprint")]
        } else {
            vec![(":lower", &"needle"), (":query", &"needle")]
        };
        expected.extend(extra.iter().map(|(name, _)| *name));
        let actual: BTreeSet<_> = (1..=statement.parameter_count())
            .map(|i| statement.parameter_name(i).unwrap())
            .collect();
        assert_eq!(actual, expected, "{query}");
        bind(&mut statement, Filter::default(), &extra).unwrap();
        statement
            .raw_query()
            .mapped(|_| Ok(()))
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
    }
}

#[test]
fn coverage_accepts_filters_without_a_query_and_keeps_byte_limits() {
    let page = crate::test_support::PageBuilder::new("Needle", "Needle evidence.")
        .build()
        .unwrap();
    let (_root, comb) = seeded_comb(&[("concepts/needle", page)]);
    let index = comb.index().unwrap();
    let empty = Search::default();
    let coverage = index.embedding_coverage(empty.filter()).unwrap();
    assert_eq!((coverage.total, coverage.pending), (1, 1));
    assert_eq!(
        index.search(&empty).err().unwrap().code,
        ErrorCode::InvalidSearch
    );
    let no_limit = Search {
        query: "needle".into(),
        limit: 0,
        ..Search::default()
    };
    assert_eq!(
        index.embedding_coverage(no_limit.filter()).unwrap().total,
        1
    );
    assert_eq!(
        index.search(&no_limit).err().unwrap().code,
        ErrorCode::InvalidSearch
    );
    for (field, limit) in [("kind", 256), ("tag", 256), ("prefix", 240)] {
        let valid = "é".repeat(limit / 2);
        let invalid = format!("{valid}x");
        for (value, accepted) in [(valid.as_str(), true), (invalid.as_str(), false)] {
            let mut filter = Filter::default();
            match field {
                "kind" => filter.kind = Some(value),
                "tag" => filter.tag = Some(value),
                _ => filter.prefix = Some(value),
            }
            let result = index.embedding_coverage(filter);
            if accepted {
                assert_eq!(result.unwrap().total, 0);
            } else {
                assert_eq!(result.unwrap_err().code, ErrorCode::InvalidSearch);
            }
        }
    }
}

fn filtered_page(kind: &str, tag: &str, at: &str) -> Page {
    Page::parse(&format!(
        "---\ntitle: Needle\ntype: {kind}\ntags: [{tag}]\nupdated_at: '{at}'\n---\nNeedle evidence."
    ))
    .unwrap()
}

fn filtered_pages() -> [(&'static str, Page); 9] {
    [
        (
            "concepts/start",
            filtered_page("concept", "focus", "2026-01-02T02:00:00+02:00"),
        ),
        (
            "concepts/middle",
            filtered_page("concept", "focus", "2026-01-02T12:00:00Z"),
        ),
        (
            "concepts/end",
            filtered_page("concept", "focus", "2026-01-03T00:00:00Z"),
        ),
        (
            "concepts/kind",
            filtered_page("note", "focus", "2026-01-02T12:00:00Z"),
        ),
        (
            "concepts/tag",
            filtered_page("concept", "other", "2026-01-02T12:00:00Z"),
        ),
        (
            "reports/prefix",
            filtered_page("concept", "focus", "2026-01-02T12:00:00Z"),
        ),
        (
            "concepts/early",
            filtered_page("concept", "focus", "2026-01-01T00:00:00Z"),
        ),
        (
            "concepts/late",
            filtered_page("concept", "focus", "2026-01-04T00:00:00Z"),
        ),
        ("concepts/undated", filtered_page("concept", "focus", "")),
    ]
}

#[test]
fn all_query_forms_apply_every_filter_and_inclusive_date_boundaries() {
    let pages = filtered_pages();
    let (_root, mut comb) = seeded_comb(&pages);
    let (index, lease) = seed_filtered_vectors(&mut comb);
    let after = "2026-01-02T00:00:00Z".parse().unwrap();
    let before = "2026-01-03T00:00:00Z".parse().unwrap();
    let filter = Filter {
        kind: Some("concept"),
        tag: Some("focus"),
        prefix: Some("concepts/"),
        after: Some(&after),
        before: Some(&before),
    };
    for query in [
        sql::FTS,
        sql::IDENTITY_SHORT,
        sql::IDENTITY_LONG,
        sql::EXACT,
        sql::VECTOR_HASH,
        sql::VECTOR_BLOB,
    ] {
        let mut statement = index.connection.prepare(query).unwrap();
        let extra: [(&str, &dyn ToSql); 2] =
            if query == sql::VECTOR_HASH || query == sql::VECTOR_BLOB {
                [(":fingerprint", &lease.fingerprint), (":lower", &"needle")]
            } else {
                [(":query", &"needle"), (":lower", &"needle")]
            };
        bind(&mut statement, filter, &extra).unwrap();
        let slugs: BTreeSet<String> = statement
            .raw_query()
            .mapped(|row| row.get(1))
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            slugs,
            BTreeSet::from([
                "concepts/start".into(),
                "concepts/middle".into(),
                "concepts/end".into()
            ]),
            "{query}"
        );
    }
    let coverage = index.embedding_coverage(filter).unwrap();
    assert_eq!(
        (
            coverage.total,
            coverage.embedded,
            coverage.pending,
            coverage.rejected
        ),
        (3, 3, 0, 0)
    );
}

fn seed_filtered_vectors(
    comb: &mut crate::Comb,
) -> (&crate::index::Index, crate::embedding::Lease) {
    let space: Space = serde_json::from_value(serde_json::json!({
        "backend":"fixture", "model":"filters", "revision":"fixed", "dimensions":2,
        "document_prefix":"", "query_prefix":"", "preprocessing":"fixture", "normalization":"l2-f32"
    }))
    .unwrap();
    comb.activate_embeddings(&space, false).unwrap();
    let index = comb.index().unwrap();
    let lease = index.embedding_lease(&space).unwrap();
    let bytes: Vec<_> = [1.0_f32, 0.0]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    index
        .seed_filter_vectors(&lease.fingerprint, &bytes)
        .unwrap();
    (index, lease)
}

#[test]
fn quoted_text_keeps_escaped_match_separate_from_raw_identity() {
    let (_root, comb) = seeded_comb(&identity_pages());
    let index = comb.index().unwrap();
    for (query, slug, evidence, safety) in [
        (
            "A \"Needle\" Phrase",
            "concepts/quoted",
            MatchEvidence::ExactTitle,
            CreateSafety::Exists,
        ),
        (
            "Quoted \"Alias\"",
            "concepts/quoted",
            MatchEvidence::Alias,
            CreateSafety::Exists,
        ),
        (
            "Needle absent",
            "concepts/quoted",
            MatchEvidence::KeywordExact,
            CreateSafety::Probable,
        ),
    ] {
        let request = Search {
            query: query.into(),
            ..Search::default()
        };
        let result = index.search(&request).unwrap();
        assert_eq!(result.results.len(), 1, "{query}");
        let hit = &result.results[0];
        assert_eq!(hit.slug.as_str(), slug, "{query}");
        assert_eq!(hit.evidence, evidence, "{query}");
        assert_eq!(hit.create_safety, safety, "{query}");
    }
}

#[test]
fn short_and_unicode_text_keep_identity_substring_routes() {
    let (_root, comb) = seeded_comb(&identity_pages());
    let index = comb.index().unwrap();
    for (query, slug, evidence, safety) in [
        (
            "eed",
            "concepts/quoted",
            MatchEvidence::IdentitySubstring,
            CreateSafety::Probable,
        ),
        (
            "ee",
            "concepts/quoted",
            MatchEvidence::IdentitySubstring,
            CreateSafety::Probable,
        ),
        (
            "語彙",
            "concepts/unicode",
            MatchEvidence::IdentitySubstring,
            CreateSafety::Probable,
        ),
    ] {
        let request = Search {
            query: query.into(),
            ..Search::default()
        };
        let result = index.search(&request).unwrap();
        assert_eq!(result.results.len(), 1, "{query}");
        let hit = &result.results[0];
        assert_eq!(hit.slug.as_str(), slug, "{query}");
        assert_eq!(hit.evidence, evidence, "{query}");
        assert_eq!(hit.create_safety, safety, "{query}");
    }
}

fn identity_pages() -> [(&'static str, Page); 2] {
    let mut quoted =
        crate::test_support::PageBuilder::new("A \"Needle\" Phrase", "Unrelated material.")
            .build()
            .unwrap();
    quoted.metadata.authored.aliases = vec!["Quoted \"Alias\"".into()];
    let unicode = crate::test_support::PageBuilder::new("語彙資料", "Unrelated material.")
        .build()
        .unwrap();
    [("concepts/quoted", quoted), ("concepts/unicode", unicode)]
}
