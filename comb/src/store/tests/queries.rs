use super::*;

#[test]
fn filters_aliases_links_and_fenced_markdown_are_deterministic() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    let text = "A [[concepts/wal]] link.\n\n```text\n---\n## Timeline\nnot evidence\n```\n\nRésumé synchronization.";
    comb.put_markdown(
        &"components/writer".parse().unwrap(),
        &markdown(text),
        None,
        &fixture_write(&Actor::Human, "writer", None).unwrap(),
    )
    .unwrap();
    assert_eq!(
        comb.get(&"components/writer".parse().unwrap())
            .unwrap()
            .page
            .compiled,
        text
    );
    assert_eq!(
        comb.index()
            .unwrap()
            .links(&"components/writer".parse().unwrap(), false)
            .unwrap(),
        vec!["concepts/wal".parse::<Slug>().unwrap()]
    );
    let hits = comb
        .index()
        .unwrap()
        .search(&Search {
            query: "WriterLoop".into(),
            tag: Some("rust".into()),
            ..Search::default()
        })
        .unwrap();
    assert_eq!(hits.results[0].evidence, MatchEvidence::Alias);
    assert!(
        comb.index()
            .unwrap()
            .search(&Search {
                query: "WriterLoop".into(),
                tag: Some("missing".into()),
                ..Search::default()
            })
            .unwrap()
            .results
            .is_empty()
    );
}

#[test]
fn substring_highlights_date_offsets_and_code_links_are_correct() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    comb.put_markdown(
        &"concepts/io".parse().unwrap(),
        &markdown("Résumé measurement. `[[concepts/inline]]`\n\n```text\n[[concepts/code]]\n```"),
        None,
        &fixture_write(&Actor::Human, "put", None).unwrap(),
    )
    .unwrap();
    let hits = comb
        .index()
        .unwrap()
        .search(&Search {
            query: "iterLoo".into(),
            ..Search::default()
        })
        .unwrap();
    assert_eq!(hits.results[0].evidence, MatchEvidence::IdentitySubstring);
    assert_ne!(hits.results[0].create_safety, CreateSafety::Exists);
    assert!(
        comb.index()
            .unwrap()
            .links(&"concepts/io".parse().unwrap(), false)
            .unwrap()
            .is_empty()
    );
    let now = chrono::Utc::now();
    let offset = chrono::FixedOffset::east_opt(12 * 3600).unwrap();
    let hits = comb
        .index()
        .unwrap()
        .search(&Search {
            query: "Résumé".into(),
            after: Some(
                (now - chrono::Duration::hours(1))
                    .with_timezone(&offset)
                    .to_rfc3339()
                    .parse()
                    .unwrap(),
            ),
            ..Search::default()
        })
        .unwrap();
    assert_eq!(hits.results.len(), 1);
    let hit = &hits.results[0];
    let range = hit.highlights[0];
    assert_eq!(&hit.snippet[range[0]..range[1]], "Résumé");
}

#[test]
fn sparse_heading_levels_and_imported_relative_links_keep_their_meaning() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    let source = tempfile::tempdir().unwrap();
    std::fs::create_dir(source.path().join("sub")).unwrap();
    std::fs::write(
        source.path().join("guide.md"),
        "## Setup\n\nStart service.\n\n## Teardown\n\nShutdownword. See [topic](sub/topic.md).",
    )
    .unwrap();
    std::fs::write(source.path().join("sub/topic.md"), "# Topic\n\nDetails.").unwrap();
    comb.import(source.path(), Some(&"operations".parse().unwrap()))
        .unwrap();
    assert_eq!(
        comb.index()
            .unwrap()
            .links(&"operations/guide".parse().unwrap(), false)
            .unwrap(),
        vec!["operations/sub/topic".parse::<Slug>().unwrap()]
    );
    assert_eq!(
        comb.index()
            .unwrap()
            .links(&"operations/sub/topic".parse().unwrap(), true)
            .unwrap(),
        vec!["operations/guide".parse::<Slug>().unwrap()]
    );
    assert_eq!(
        comb.index()
            .unwrap()
            .search(&Search {
                query: "Shutdownword".into(),
                ..Search::default()
            })
            .unwrap()
            .results[0]
            .heading,
        "Teardown"
    );
}

#[test]
fn unicode_titles_have_consistent_exact_and_short_identity_matching() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    comb.put_markdown(
        &"concepts/unicode".parse().unwrap(),
        "---\ntitle: РЕЗЮМЕ ÉTUDES\n---\nBody",
        None,
        &fixture_write(&Actor::Human, "create", None).unwrap(),
    )
    .unwrap();
    let exact = comb
        .index()
        .unwrap()
        .search(&Search {
            query: "резюме études".into(),
            ..Search::default()
        })
        .unwrap();
    assert_eq!(exact.results[0].create_safety, CreateSafety::Exists);
    let short = comb
        .index()
        .unwrap()
        .search(&Search {
            query: "ре".into(),
            ..Search::default()
        })
        .unwrap();
    assert_eq!(short.results[0].slug, "concepts/unicode".parse().unwrap());
}

#[test]
fn graph_accepts_one_thousand_nodes_with_back_edges() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    comb.put_markdown(
        &"concepts/root".parse().unwrap(),
        &markdown("Root"),
        None,
        &fixture_write(&Actor::Human, "root", None).unwrap(),
    )
    .unwrap();
    // The graph operation is index-only beyond its origin. Construct a large
    // deterministic graph directly, avoiding 1,000 unrelated durable file writes.
    comb.index.seed_graph_with_back_edges().unwrap();
    assert_eq!(comb.graph("concepts/root", 2).unwrap().len(), 1000);
    comb.index.add_graph_overflow_neighbor().unwrap();
    assert_eq!(
        comb.graph("concepts/root", 2).unwrap_err().code,
        crate::ErrorCode::GraphLimit
    );
}
