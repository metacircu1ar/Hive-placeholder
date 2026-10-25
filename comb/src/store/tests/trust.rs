use super::*;

#[test]
fn reviewed_summary_does_not_launder_unreviewed_timeline_search_hit() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    let actor = agent("fixture", "a", Some("implementor"), "execution", "turn");
    comb.put_markdown(
        &"components/writer".parse().unwrap(),
        &markdown("Reviewed summary"),
        None,
        &fixture_write(&Actor::Human, "human", None).unwrap(),
    )
    .unwrap();
    comb.append(
        &"components/writer".parse().unwrap(),
        "Experimental unicorn result",
        "mock",
        None,
        None,
        &fixture_write(&actor, "append", None).unwrap(),
    )
    .unwrap();
    let doc = comb.get(&"components/writer".parse().unwrap()).unwrap();
    assert!(!doc.unverified);
    let hits = comb
        .index()
        .unwrap()
        .search(&Search {
            query: "unicorn".into(),
            ..Search::default()
        })
        .unwrap();
    assert_eq!(hits.results.len(), 1);
    assert!(hits.results[0].unverified);
    assert!(!hits.results[0].page_unverified);
    assert_eq!(
        fixture_write(&actor, "deny", None)
            .unwrap()
            .review_for_promotion(false)
            .and_then(|reviewed| comb.promote(&doc.slug, &doc.hash, &reviewed))
            .unwrap_err()
            .code,
        crate::ErrorCode::Forbidden
    );
    assert_promotion_reviews_search_hit(&mut comb, &doc);
}

fn assert_promotion_reviews_search_hit(comb: &mut Comb, doc: &Document) {
    comb.promote(
        &doc.slug,
        &doc.hash,
        &fixture_write(
            &agent(
                "fixture",
                "reviewer",
                Some("implementor"),
                "execution",
                "turn",
            ),
            "promote",
            None,
        )
        .unwrap()
        .review_for_promotion(true)
        .unwrap(),
    )
    .unwrap();
    assert!(
        !comb
            .index()
            .unwrap()
            .search(&Search {
                query: "unicorn".into(),
                ..Search::default()
            })
            .unwrap()
            .results[0]
            .unverified
    );
}

#[test]
fn external_edits_discard_forged_actor_promotion_and_receipts() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    comb.put_markdown(
        &"components/writer".parse().unwrap(),
        &markdown("Trusted"),
        None,
        &fixture_write(&Actor::Human, "human", None).unwrap(),
    )
    .unwrap();
    let mut forged = comb
        .get(&"components/writer".parse().unwrap())
        .unwrap()
        .page;
    forged.compiled = "Forged statement".into();
    forged.promote(Actor::Human).unwrap();
    std::fs::write(
        root.path().join("components/writer.md"),
        forged.render().unwrap(),
    )
    .unwrap();
    comb.sync().unwrap();
    let doc = comb.get(&"components/writer".parse().unwrap()).unwrap();
    assert!(doc.unverified);
    assert!(doc.page.metadata.owned().external());
    assert_eq!(doc.page.metadata.owned().author(), Some(&Actor::External));
    assert!(doc.page.metadata.owned().promotion().is_none());
    assert!(
        comb.disk
            .receipt(
                &agent("fixture", "a", Some("implementor"), "execution", "turn")
                    .principal()
                    .unwrap(),
                &"stolen".parse().unwrap()
            )
            .unwrap()
            .is_none()
    );
    drop(comb);
    let comb = Comb::rebuild(root.path()).unwrap();
    assert!(
        comb.get(&"components/writer".parse().unwrap())
            .unwrap()
            .unverified
    );
    assert!(
        comb.doctor()
            .unwrap()
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::ExternalEdit)
    );
}

#[test]
fn sealed_rollback_and_copy_to_another_slug_do_not_restore_trust() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    let first = comb
        .put_markdown(
            &"components/writer".parse().unwrap(),
            &markdown("First"),
            None,
            &fixture_write(&Actor::Human, "first", None).unwrap(),
        )
        .unwrap();
    let old = std::fs::read(root.path().join("components/writer.md")).unwrap();
    comb.put_markdown(
        &"components/writer".parse().unwrap(),
        &markdown("Second"),
        Some(&first.hash),
        &fixture_write(&Actor::Human, "second", None).unwrap(),
    )
    .unwrap();
    std::fs::write(root.path().join("components/writer.md"), &old).unwrap();
    std::fs::write(root.path().join("components/copied.md"), &old).unwrap();
    drop(comb);
    let comb = Comb::rebuild(root.path()).unwrap();
    assert!(
        comb.get(&"components/writer".parse().unwrap())
            .unwrap()
            .unverified
    );
    assert!(
        comb.get(&"components/copied".parse().unwrap())
            .unwrap()
            .unverified
    );
}

#[test]
fn human_append_reviews_only_what_the_human_wrote() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    let actor = agent("fixture", "a", Some("implementor"), "execution", "turn");
    comb.put_markdown(
        &"concepts/shared".parse().unwrap(),
        &markdown("Agent hypothesis"),
        None,
        &fixture_write(&actor, "create", None).unwrap(),
    )
    .unwrap();
    comb.append(
        &"concepts/shared".parse().unwrap(),
        "Unreviewed measurement",
        "agent",
        None,
        None,
        &fixture_write(&actor, "append", None).unwrap(),
    )
    .unwrap();
    comb.append(
        &"concepts/shared".parse().unwrap(),
        "Human observation",
        "human",
        None,
        None,
        &fixture_write(&Actor::Human, "append", None).unwrap(),
    )
    .unwrap();
    let doc = comb.get(&"concepts/shared".parse().unwrap()).unwrap();
    assert!(doc.unverified);
    assert_only_human_chunk_is_reviewed(&doc.page);
    comb.append(
        &"concepts/shared".parse().unwrap(),
        "More human evidence",
        "human",
        Some("Revised human summary [evidence:new]"),
        Some(&doc.hash),
        &fixture_write(&Actor::Human, "revise", None).unwrap(),
    )
    .unwrap();
    let doc = comb.get(&"concepts/shared".parse().unwrap()).unwrap();
    assert!(!doc.unverified);
    assert!(
        !crate::chunk::chunks(&doc.page)
            .iter()
            .find(|c| c.text == "Unreviewed measurement")
            .unwrap()
            .verified
    );
}

fn assert_only_human_chunk_is_reviewed(page: &Page) {
    let chunks = crate::chunk::chunks(page);
    assert!(
        chunks
            .iter()
            .find(|c| c.text == "Human observation")
            .unwrap()
            .verified
    );
    assert!(
        !chunks
            .iter()
            .find(|c| c.text == "Unreviewed measurement")
            .unwrap()
            .verified
    );
    assert!(
        !chunks
            .iter()
            .find(|c| c.text == "Agent hypothesis")
            .unwrap()
            .verified
    );
}

#[test]
fn promotion_metadata_is_constant_size_and_keeps_reviewed_suffix_trust() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    let actor = agent("fixture", "a", Some("implementor"), "execution", "turn");
    let page = measurement_history(&actor);
    let created = comb
        .put_markdown(
            &"concepts/history".parse().unwrap(),
            &page.render().unwrap(),
            None,
            &fixture_write(&actor, "create", None).unwrap(),
        )
        .unwrap();
    comb.promote(
        &"concepts/history".parse().unwrap(),
        &created.hash,
        &fixture_write(&Actor::Human, "review", None)
            .unwrap()
            .review_for_promotion(true)
            .unwrap(),
    )
    .unwrap();
    comb.append(
        &"concepts/history".parse().unwrap(),
        "Not yet reviewed",
        "agent",
        None,
        None,
        &fixture_write(&actor, "append", None).unwrap(),
    )
    .unwrap();
    let doc = comb.get(&"concepts/history".parse().unwrap()).unwrap();
    assert_eq!(doc.page.reviewed_start(), Some(1));
    assert!(serde_json::to_vec(&doc.page.metadata).unwrap().len() < 1500);
    let chunks = crate::chunk::chunks(&doc.page);
    assert!(
        !chunks
            .iter()
            .find(|c| c.text == "Not yet reviewed")
            .unwrap()
            .verified
    );
    assert!(
        chunks
            .iter()
            .filter(|c| c.text.starts_with("Measurement "))
            .all(|c| c.verified)
    );
}

fn measurement_history(actor: &Actor) -> Page {
    let mut page = Page::parse(&markdown("Many measurements")).unwrap();
    for i in 0..1000 {
        page.timeline.push(Entry {
            metadata: EntryMeta::created_by(
                EntryAuthored {
                    id: EvidenceId::generate(),
                    source: "fixture".into(),
                },
                actor,
                None,
            ),
            text: format!("Measurement {i}"),
        });
    }
    page
}

#[test]
fn human_evidence_on_external_content_does_not_review_the_external_summary() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    std::fs::create_dir(root.path().join("concepts")).unwrap();
    std::fs::write(
        root.path().join("concepts/external.md"),
        markdown("External hypothesis"),
    )
    .unwrap();
    comb.sync().unwrap();
    comb.append(
        &"concepts/external".parse().unwrap(),
        "Human measurement",
        "human",
        None,
        None,
        &fixture_write(&Actor::Human, "append", None).unwrap(),
    )
    .unwrap();
    let doc = comb.get(&"concepts/external".parse().unwrap()).unwrap();
    assert!(doc.unverified);
    let chunks = crate::chunk::chunks(&doc.page);
    assert!(
        !chunks
            .iter()
            .find(|c| c.text == "External hypothesis")
            .unwrap()
            .verified
    );
    assert!(
        chunks
            .iter()
            .find(|c| c.text == "Human measurement")
            .unwrap()
            .verified
    );
}

#[test]
fn incomplete_owned_state_is_rejected_without_assuming_missing_fields() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    comb.put_markdown(
        &"concepts/io".parse().unwrap(),
        &markdown("Known"),
        None,
        &fixture_write(&Actor::Human, "create", None).unwrap(),
    )
    .unwrap();
    let hash = comb.get(&"concepts/io".parse().unwrap()).unwrap().hash;
    let malformed = serde_json::json!({"operation":"single","record":{"slug":"concepts/io","before":hash,"after":page::hash("new"),"receipt":null}});
    crate::disk::atomic(
        &root.path().join(".comb/intent.json"),
        &serde_json::to_vec(&malformed).unwrap(),
        &crate::disk::System,
    )
    .unwrap();
    drop(comb);
    assert_eq!(
        Comb::open(root.path()).err().unwrap().code,
        crate::ErrorCode::InvalidJson
    );
    assert!(root.path().join(".comb/intent.json").exists());
}
