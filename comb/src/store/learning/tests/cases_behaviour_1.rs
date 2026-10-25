//! Behaviour regressions.
use super::*;

#[test]
fn interrupted_marking_replays_after_rebuild() {
    let (directory, mut comb, note) = setup();
    let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
    let first = interrupt_first_copy(&mut comb, &note, &actor);
    drop(comb);
    let mut comb = Comb::rebuild(directory.path()).unwrap();
    let replay = comb
        .consolidate(
            &first,
            &fixture_write(&actor, "interrupted", Some("curation-1")).unwrap(),
        )
        .unwrap();
    assert!(replay.target.replayed);
    assert!(replay.note.replayed);
    assert_eq!(
        comb.get(&"concepts/writer".parse().unwrap())
            .unwrap()
            .page
            .timeline
            .len(),
        1
    );
    assert_eq!(comb.stats().unwrap().inbox, 0);
}

#[test]
fn rewritten_source_or_missing_target_history_requires_explicit_recorded_recopy() {
    for source in [false, true] {
        let (directory, mut comb, note) = setup();
        let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
        comb.consolidate(
            &request(&comb, &note),
            &fixture_write(&actor, "first", Some("curation-1")).unwrap(),
        )
        .unwrap();
        hand_edit(
            directory.path(),
            &mut comb,
            if source { &note } else { "concepts/writer" },
            |page| {
                if source {
                    page.timeline[0].text = "Rewritten historical measurement".into();
                } else {
                    page.timeline.clear();
                }
            },
        );
        let next = request(&comb, &note);
        assert_eq!(
            comb.consolidate(
                &next,
                &fixture_write(&actor, "refused", Some("curation-1")).unwrap()
            )
            .unwrap_err()
            .code,
            crate::ErrorCode::ConsolidationConflict
        );
        assert_eq!(
            comb.get(&note.parse().unwrap()).unwrap().hash,
            next.note_hash
        );
        assert_eq!(
            comb.get(&"concepts/writer".parse().unwrap()).unwrap().hash,
            next.target_hash
        );
        assert_recorded_recopy(&mut comb, &note, &actor, next);
    }
}
