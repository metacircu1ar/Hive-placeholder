use super::*;

#[test]
fn capture_skips_blank_title_lines_and_preserves_evidence_and_replay() {
    let (_dir, mut comb) = TestComb::new().unwrap().into_parts();
    let text = "\n  \t\r\nObservation\nDetails stay intact.";
    let write = Write::new(&Actor::Human, &"blank-lines".parse().unwrap(), None).unwrap();
    let created = comb.capture(text, &write).unwrap();
    let document = comb.get(&created.slug).unwrap();
    assert_eq!(document.page.metadata.authored.title, "Observation");
    assert_eq!(document.page.compiled, text.trim());
    let replay = comb.capture(text, &write).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.hash, created.hash);
    let indented = format!("{}Observation", " ".repeat(120));
    let created = comb
        .capture(&indented, &write.with_request(&"indented".parse().unwrap()))
        .unwrap();
    assert_eq!(
        comb.get(&created.slug)
            .unwrap()
            .page
            .metadata
            .authored
            .title,
        "Note"
    );
    let error = comb.capture(" \n\t", &write).unwrap_err();
    assert_eq!(error.input().unwrap().field(), "/operation/text");
}

#[test]
fn two_mock_agents_use_cas_and_atomic_evidence_summary_updates() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    let a = agent("fixture", "a", Some("implementor"), "execution", "turn");
    let b = agent("fixture", "b", Some("implementor"), "execution", "turn");
    let initial = comb
        .put_markdown(
            &"components/writer".parse().unwrap(),
            &markdown("Initial summary"),
            None,
            &fixture_write(&a, "create", None).unwrap(),
        )
        .unwrap();
    let first = comb
        .append(
            &"components/writer".parse().unwrap(),
            "Observed WAL stalls",
            "commit:a",
            Some("WAL synchronization explains stalls"),
            Some(&initial.hash),
            &fixture_write(&a, "a-update", None).unwrap(),
        )
        .unwrap();
    let error = comb
        .append(
            &"components/writer".parse().unwrap(),
            "Different explanation",
            "commit:b",
            Some("Overwrite"),
            Some(&initial.hash),
            &fixture_write(&b, "stale-update", None).unwrap(),
        )
        .unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::Conflict);
    comb.append(
        &"components/writer".parse().unwrap(),
        "Independent measurement",
        "commit:b",
        None,
        None,
        &fixture_write(&b, "b-append", None).unwrap(),
    )
    .unwrap();
    let doc = comb.get(&"components/writer".parse().unwrap()).unwrap();
    assert_eq!(doc.page.compiled, "WAL synchronization explains stalls");
    assert_eq!(doc.page.timeline.len(), 2);
    assert_eq!(doc.page.timeline[0].metadata.owned().actor(), &b);
    assert_eq!(doc.page.timeline[1].metadata.owned().actor(), &a);
    assert_ne!(doc.hash, first.hash);
}

#[test]
fn full_put_cannot_rewrite_existing_evidence() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    let actor = agent("fixture", "a", Some("implementor"), "execution", "turn");
    comb.put_markdown(
        &"components/writer".parse().unwrap(),
        &markdown("Summary"),
        None,
        &fixture_write(&actor, "put", None).unwrap(),
    )
    .unwrap();
    comb.append(
        &"components/writer".parse().unwrap(),
        "Original evidence",
        "test",
        None,
        None,
        &fixture_write(&actor, "append", None).unwrap(),
    )
    .unwrap();
    let mut doc = comb.get(&"components/writer".parse().unwrap()).unwrap();
    doc.page.timeline[0].text = "Changed history".into();
    assert_eq!(
        comb.put_markdown(
            &doc.slug,
            &doc.page.render().unwrap(),
            Some(&doc.hash),
            &fixture_write(&actor, "rewrite", None).unwrap()
        )
        .unwrap_err()
        .code,
        crate::ErrorCode::ImmutableTimeline
    );
}

#[test]
fn whitespace_and_html_sources_roundtrip_with_atomic_citations() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    let actor = agent("fixture", "a", Some("implementor"), "execution", "turn");
    let created = comb
        .put_markdown(
            &"concepts/io".parse().unwrap(),
            &markdown("Initial"),
            None,
            &fixture_write(&actor, "put", None).unwrap(),
        )
        .unwrap();
    comb.append(
        &"concepts/io".parse().unwrap(),
        "Measured file sync",
        "command: echo '-->'",
        Some("\n  Sync costs time. [evidence:new]\n"),
        Some(&created.hash),
        &fixture_write(&actor, "append", None).unwrap(),
    )
    .unwrap();
    drop(comb);
    let comb = Comb::open(root.path()).unwrap();
    let page = comb.get(&"concepts/io".parse().unwrap()).unwrap().page;
    assert_eq!(
        page.timeline[0].metadata.authored.source,
        "command: echo '-->'"
    );
    assert_eq!(page.timeline[0].metadata.owned().actor(), &actor);
    assert!(page.compiled.contains(&format!(
        "[evidence:{}]",
        page.timeline[0].metadata.authored.id
    )));
    assert!(!comb.doctor().unwrap().findings.iter().any(|f| f.kind
        == FindingKind::DanglingCitation
        || f.kind == FindingKind::UncitedParagraph));
}

#[test]
fn malformed_markdown_is_rejected_before_committing_and_writer_remains_usable() {
    let (_root, mut comb) = TestComb::new().unwrap().into_parts();
    let actor = agent("fixture", "a", Some("implementor"), "execution", "turn");
    comb.put_markdown(
        &"concepts/io".parse().unwrap(),
        &markdown("Initial"),
        None,
        &fixture_write(&actor, "put", None).unwrap(),
    )
    .unwrap();
    let before = comb.get(&"concepts/io".parse().unwrap()).unwrap();
    assert!(
        comb.append(
            &"concepts/io".parse().unwrap(),
            "Measurement",
            "source",
            Some("```unclosed"),
            Some(&before.hash),
            &fixture_write(&actor, "invalid", None).unwrap()
        )
        .is_err()
    );
    assert_eq!(
        comb.get(&"concepts/io".parse().unwrap()).unwrap().hash,
        before.hash
    );
    comb.append(
        &"concepts/io".parse().unwrap(),
        "Good measurement",
        "source",
        None,
        None,
        &fixture_write(&actor, "valid", None).unwrap(),
    )
    .unwrap();
    assert_eq!(
        comb.get(&"concepts/io".parse().unwrap())
            .unwrap()
            .page
            .timeline
            .len(),
        1
    );
}
