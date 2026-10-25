//! Presentation regressions.
use super::*;

#[test]
fn decisions_require_reviewer_and_subsequent_edits_reopen_the_proposal() {
    let (_directory, mut comb, _) = setup();
    let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
    let page = comb
        .put_markdown(
            &"review/duplicate".parse().unwrap(),
            "---\ntitle: Possible duplicate\n---\nCompare the two pages.",
            None,
            &fixture_write(&actor, "proposal", Some("curation-1")).unwrap(),
        )
        .unwrap();
    assert_eq!(comb.stats().unwrap().unresolved_proposals, 1);
    assert_eq!(
        fixture_write(&actor, "reject", Some("curation-1"))
            .unwrap()
            .review_for_decision(false)
            .and_then(|reviewed| comb.decide(
                &"review/duplicate".parse().unwrap(),
                &page.hash,
                Decision::Reject,
                "Different evidence",
                &reviewed
            ))
            .unwrap_err()
            .code,
        crate::ErrorCode::Forbidden
    );
    comb.decide(
        &"review/duplicate".parse().unwrap(),
        &page.hash,
        Decision::Reject,
        "Different evidence",
        &fixture_write(&actor, "reject", Some("curation-1"))
            .unwrap()
            .review_for_decision(true)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(comb.stats().unwrap().unresolved_proposals, 0);
    assert!(
        comb.decide(
            &"review/duplicate".parse().unwrap(),
            &page.hash,
            Decision::Reject,
            "Different evidence",
            &fixture_write(&actor, "reject", Some("curation-1"))
                .unwrap()
                .review_for_decision(true)
                .unwrap()
        )
        .unwrap()
        .replayed
    );
    assert_edit_reopens_proposal(&mut comb, &actor);
}
