//! Recovery regressions.
use super::*;

#[test]
fn source_receipt_collision_rejects_before_writes_and_does_not_block_reopen() {
    let (directory, mut comb, note) = setup();
    let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
    let outer_id = "colliding-consolidation";
    let mark_id = page::hash(serde_json::to_vec(&("consolidation-source", outer_id)).unwrap());
    comb.capture(
        "Unrelated ordinary request",
        &fixture_write(&actor, &mark_id, Some("curation-1")).unwrap(),
    )
    .unwrap();
    let request = request(&comb, &note);
    let error = comb
        .consolidate(
            &request,
            &fixture_write(&actor, outer_id, Some("curation-1")).unwrap(),
        )
        .unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::RequestConflict);
    assert_eq!(
        comb.get(&note.parse().unwrap()).unwrap().hash,
        request.note_hash
    );
    assert_eq!(comb.get(&request.target).unwrap().hash, request.target_hash);
    drop(comb);
    let comb = Comb::open(directory.path()).unwrap();
    assert_eq!(
        comb.get(&note.parse().unwrap()).unwrap().hash,
        request.note_hash
    );
    assert_eq!(comb.get(&request.target).unwrap().hash, request.target_hash);
}

#[test]
fn source_preflight_failure_leaves_both_pages_and_receipts_unchanged() {
    let (_directory, mut comb, note) = setup();
    let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
    let first = request(&comb, &note);
    comb.consolidate(
        &first,
        &fixture_write(&actor, "initial-coverage", Some("curation-1")).unwrap(),
    )
    .unwrap();
    let request = request(&comb, &note);
    // Reuse existing target coverage: the large run must enter only the source
    // checkpoint, never a newly copied target evidence entry.
    let run = "r".repeat(page::PAGE_BYTES);
    let write = Write::new(&actor, &"source-preflight".parse().unwrap(), Some(&run)).unwrap();
    let source = comb.get(&request.note).unwrap();
    let mut target = comb.get(&request.target).unwrap();
    let prior_timeline = target.page.timeline.clone();
    let evidence = compile(&source, &mut target.page, &request, &write).unwrap();
    assert_eq!(target.page.timeline, prior_timeline);
    let prepare = |slug: &Slug, before: &ContentHash, page: Page| {
        PreparedPage::new(
            &comb.disk,
            slug,
            before,
            page,
            crate::page::Receipt {
                principal: write.principal.clone(),
                request_id: write.request_id.clone(),
                fingerprint: "ab".repeat(32),
                operation: "consolidate".into(),
                slug: slug.clone(),
            },
        )
    };
    prepare(&request.target, &request.target_hash, target.page.clone()).unwrap();
    let mut marked = source.page;
    marked
        .checkpoint(&request.target, &target.page, &evidence, &actor, &run)
        .unwrap();
    assert_eq!(
        prepare(&request.note, &request.note_hash, marked)
            .unwrap_err()
            .code,
        crate::ErrorCode::PageLimit
    );
    let error = comb.consolidate(&request, &write).unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::PageLimit);
    assert!(!error.file_committed);
    assert_eq!(comb.get(&request.note).unwrap().hash, request.note_hash);
    assert_eq!(comb.get(&request.target).unwrap().hash, request.target_hash);
    assert!(
        comb.disk
            .receipt(&write.principal, &write.request_id)
            .unwrap()
            .is_none()
    );
    assert!(!comb.disk.admin.join("intent.json").exists());
    comb.ready().unwrap();
}
