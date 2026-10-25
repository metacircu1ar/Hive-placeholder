//! Learning regressions.
use super::*;

#[test]
fn successful_consolidation_replays_after_rebuild_and_copies_only_new_evidence() {
    let (directory, mut comb, note) = setup();
    let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
    let original = comb.get(&note.parse().unwrap()).unwrap().markdown;
    let first = request(&comb, &note);
    comb.consolidate(
        &first,
        &fixture_write(&actor, "fold-1", Some("curation-1")).unwrap(),
    )
    .unwrap();
    assert_first_copy(&comb, &original);
    drop(comb);
    let mut comb = Comb::rebuild(directory.path()).unwrap();
    let replay = comb
        .consolidate(
            &first,
            &fixture_write(&actor, "fold-1", Some("curation-1")).unwrap(),
        )
        .unwrap();
    assert!(replay.target.replayed && replay.note.replayed);
    for i in 0..2 {
        comb.append(
            &note.parse().unwrap(),
            &format!("New measurement {i}"),
            "test:later",
            None,
            None,
            &fixture_write(&actor, &format!("append-{i}"), Some("curation-1")).unwrap(),
        )
        .unwrap();
    }
    assert_eq!(comb.stats().unwrap().inbox, 1);
    comb.consolidate(
        &request(&comb, &note),
        &fixture_write(&actor, "fold-2", Some("curation-1")).unwrap(),
    )
    .unwrap();
    let copied = copy(&comb);
    assert!(copied.original_markdown.is_none() && copied.compiled.is_none());
    assert_eq!(copied.entries.len(), 2);
    assert!(
        copied
            .entries
            .iter()
            .all(|e| e.text.starts_with("New measurement"))
    );
    assert_eq!(
        comb.get(&"concepts/writer".parse().unwrap())
            .unwrap()
            .page
            .timeline
            .len(),
        2
    );
    assert_eq!(comb.stats().unwrap().inbox, 0);
}

#[test]
fn source_edit_after_interruption_uses_target_coverage() {
    let (directory, mut comb, note) = setup();
    let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
    let first = interrupt_first_copy(&mut comb, &note, &actor);
    drop(comb);
    let mut comb = Comb::rebuild(directory.path()).unwrap();
    comb.append(
        &note.parse().unwrap(),
        "New evidence after interruption",
        "test:after",
        None,
        None,
        &fixture_write(&actor, "after", Some("curation-1")).unwrap(),
    )
    .unwrap();
    let replay = comb
        .consolidate(
            &first,
            &fixture_write(&actor, "interrupted", Some("curation-1")).unwrap(),
        )
        .unwrap();
    assert!(replay.target.replayed && replay.note.replayed);
    assert_eq!(
        comb.get(&"concepts/writer".parse().unwrap())
            .unwrap()
            .page
            .timeline
            .len(),
        1
    );
    comb.consolidate(
        &request(&comb, &note),
        &fixture_write(&actor, "revised", Some("curation-1")).unwrap(),
    )
    .unwrap();
    let copied = copy(&comb);
    assert!(copied.original_markdown.is_none());
    assert_eq!(copied.entries.len(), 1);
    assert_eq!(copied.entries[0].text, "New evidence after interruption");
    assert_eq!(comb.stats().unwrap().inbox, 0);
}

#[test]
fn source_summary_changes_reopen_backlog_without_copying_old_timeline() {
    let (_directory, mut comb, note) = setup();
    let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
    comb.consolidate(
        &request(&comb, &note),
        &fixture_write(&actor, "first", Some("curation-1")).unwrap(),
    )
    .unwrap();
    let mut source = comb.get(&note.parse().unwrap()).unwrap();
    source.page.compiled = "Revised interpretation".into();
    comb.put_markdown(
        &note.parse().unwrap(),
        &source.page.render().unwrap(),
        Some(&source.hash),
        &fixture_write(&actor, "edit", Some("curation-1")).unwrap(),
    )
    .unwrap();
    assert_eq!(comb.stats().unwrap().inbox, 1);
    comb.consolidate(
        &request(&comb, &note),
        &fixture_write(&actor, "second", Some("curation-1")).unwrap(),
    )
    .unwrap();
    let copied = copy(&comb);
    assert_eq!(copied.compiled.unwrap().summary, "Revised interpretation");
    assert!(copied.entries.is_empty() && copied.original_markdown.is_none());
}

#[test]
fn source_hand_edit_preserves_incremental_content_without_restoring_authorship() {
    let (directory, mut comb, note) = setup();
    let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
    comb.consolidate(
        &request(&comb, &note),
        &fixture_write(&actor, "first", Some("curation-1")).unwrap(),
    )
    .unwrap();
    hand_edit(directory.path(), &mut comb, &note, |page| {
        page.compiled = "Corrected interpretation".into();
    });
    assert!(
        comb.get(&note.parse().unwrap())
            .unwrap()
            .page
            .metadata
            .owned()
            .consolidated_into()
            .is_empty()
    );
    comb.consolidate(
        &request(&comb, &note),
        &fixture_write(&actor, "corrected", Some("curation-1")).unwrap(),
    )
    .unwrap();
    let copied = copy(&comb);
    assert!(copied.original_markdown.is_none() && copied.entries.is_empty() && !copied.recopy);
    assert_eq!(copied.compiled.unwrap().summary, "Corrected interpretation");
    assert!(
        comb.get(&note.parse().unwrap())
            .unwrap()
            .page
            .timeline
            .iter()
            .all(|entry| entry.metadata.owned().actor() == &Actor::External)
    );
    assert_eq!(comb.stats().unwrap().inbox, 0);
}

#[test]
fn new_note_can_be_consolidated_into_a_hand_edited_target_without_recopy() {
    let (directory, mut comb, note) = setup();
    let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
    comb.consolidate(
        &request(&comb, &note),
        &fixture_write(&actor, "first", Some("curation-1")).unwrap(),
    )
    .unwrap();
    hand_edit(directory.path(), &mut comb, "concepts/writer", |page| {
        page.compiled = "Human clarification".into();
    });
    let new = comb
        .capture(
            "A distinct new observation",
            &fixture_write(&actor, "new-note", Some("curation-1")).unwrap(),
        )
        .unwrap()
        .slug;
    let original = comb.get(&new).unwrap().markdown;
    comb.consolidate(
        &request(&comb, new.as_str()),
        &fixture_write(&actor, "new-copy", Some("curation-1")).unwrap(),
    )
    .unwrap();
    let copied = copy(&comb);
    assert!(!copied.recopy);
    assert_eq!(copied.original_markdown.as_deref(), Some(original.as_str()));
    let target = comb.get(&"concepts/writer".parse().unwrap()).unwrap();
    assert_eq!(target.page.timeline.len(), 2);
    assert_eq!(
        target.page.timeline[1].metadata.owned().actor(),
        &Actor::External
    );
}

#[test]
fn lost_coverage_after_interrupted_first_copy_or_both_external_edits_requires_recopy() {
    for interrupted in [false, true] {
        let (directory, mut comb, note) = setup();
        let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
        if interrupted {
            INTERRUPT_AFTER_TARGET.with(|flag| flag.set(true));
        }
        let first = comb.consolidate(
            &request(&comb, &note),
            &fixture_write(&actor, "first", Some("curation-1")).unwrap(),
        );
        if interrupted {
            assert!(first.unwrap_err().file_committed);
            assert_eq!(
                comb.ready().unwrap_err().code,
                crate::ErrorCode::RecoveryRequired
            );
            drop(comb);
            comb = Comb::open(directory.path()).unwrap();
        } else {
            first.unwrap();
        }
        hand_edit(directory.path(), &mut comb, &note, |page| {
            page.compiled = "Edited source".into();
        });
        hand_edit(directory.path(), &mut comb, "concepts/writer", |page| {
            page.compiled = "Edited target".into();
        });
        assert_lost_coverage_requires_recopy(&mut comb, &note, &actor);
    }
}

#[test]
fn target_hand_edit_uses_sealed_source_checkpoint_for_increment_and_interrupted_retry() {
    for appended in [false, true] {
        let (directory, mut comb, note) = setup();
        let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
        comb.consolidate(
            &request(&comb, &note),
            &fixture_write(&actor, "first", Some("curation-1")).unwrap(),
        )
        .unwrap();
        hand_edit(directory.path(), &mut comb, "concepts/writer", |page| {
            page.compiled = "Corrected target summary".into();
        });
        if appended {
            comb.append(
                &note.parse().unwrap(),
                "Second measurement",
                "test:new",
                None,
                None,
                &fixture_write(&actor, "append", Some("curation-1")).unwrap(),
            )
            .unwrap();
        }
        let next = request(&comb, &note);
        INTERRUPT_AFTER_TARGET.with(|flag| flag.set(true));
        assert!(
            comb.consolidate(
                &next,
                &fixture_write(&actor, "second", Some("curation-1")).unwrap()
            )
            .unwrap_err()
            .file_committed
        );
        assert_eq!(
            comb.ready().unwrap_err().code,
            crate::ErrorCode::RecoveryRequired
        );
        drop(comb);
        let mut comb = Comb::rebuild(directory.path()).unwrap();
        let result = comb
            .consolidate(
                &next,
                &fixture_write(&actor, "second", Some("curation-1")).unwrap(),
            )
            .unwrap();
        assert!(result.target.replayed && result.note.replayed);
        assert_retried_checkpoint(&comb, appended);
    }
}

#[test]
fn oversized_summary_and_invalid_witness_cannot_silently_advance_coverage() {
    let (_directory, mut comb, note) = setup();
    let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
    let mut over = request(&comb, &note);
    over.summary = "x".repeat(page::PAGE_BYTES);
    assert_eq!(
        comb.consolidate(
            &over,
            &fixture_write(&actor, "oversize", Some("curation-1")).unwrap()
        )
        .unwrap_err()
        .code,
        crate::ErrorCode::PageLimit
    );
    assert_eq!(
        comb.get(&note.parse().unwrap()).unwrap().hash,
        over.note_hash
    );
    assert_eq!(
        comb.get(&"concepts/writer".parse().unwrap()).unwrap().hash,
        over.target_hash
    );
    comb.consolidate(
        &request(&comb, &note),
        &fixture_write(&actor, "first", Some("curation-1")).unwrap(),
    )
    .unwrap();
    let mut target = comb.get(&"concepts/writer".parse().unwrap()).unwrap();
    crate::golden::edit_record(&mut target.page.timeline[0].metadata, |raw| {
        raw["consolidation"]["content"]["timeline_digest"] = serde_json::json!("invalid");
    });
    comb.disk
        .seal(&"concepts/writer".parse().unwrap(), &mut target.page)
        .unwrap();
    comb.persist(
        &"concepts/writer".parse().unwrap(),
        &target.page,
        Some(&target.hash),
        None,
    )
    .unwrap();
    let before = comb.get(&"concepts/writer".parse().unwrap()).unwrap().hash;
    assert_eq!(
        comb.consolidate(
            &request(&comb, &note),
            &fixture_write(&actor, "bad-witness", Some("curation-1")).unwrap()
        )
        .unwrap_err()
        .code,
        crate::ErrorCode::ConsolidationConflict
    );
    assert_eq!(
        comb.get(&"concepts/writer".parse().unwrap()).unwrap().hash,
        before
    );
}
