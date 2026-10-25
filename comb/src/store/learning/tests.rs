use super::*;
use crate::Actor;
use crate::test_support::{TestComb, agent, write as fixture_write};

fn setup() -> (tempfile::TempDir, Comb, String) {
    let (directory, mut comb) = TestComb::new().unwrap().into_parts();
    let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
    let note = comb
        .capture(
            "Observed database contention.\n\n```text\n~~~\n<!-- illustrative marker -->\n```",
            &fixture_write(&actor, "capture", Some("curation-1")).unwrap(),
        )
        .unwrap()
        .slug;
    comb.append(
        &note,
        "First measurement",
        "test:initial",
        None,
        None,
        &fixture_write(&actor, "initial-evidence", Some("curation-1")).unwrap(),
    )
    .unwrap();
    comb.put_markdown(
        &"concepts/writer".parse().unwrap(),
        "---\ntitle: Writer\n---\nOriginal summary",
        None,
        &fixture_write(&actor, "target", Some("curation-1")).unwrap(),
    )
    .unwrap();
    (directory, comb, note.to_string())
}
fn request(comb: &Comb, note: &str) -> Consolidate {
    Consolidate {
        note: note.parse().unwrap(),
        target: "concepts/writer".parse().unwrap(),
        note_hash: comb.get(&note.parse().unwrap()).unwrap().hash,
        target_hash: comb.get(&"concepts/writer".parse().unwrap()).unwrap().hash,
        summary: "Measured writer contention. [evidence:new]".into(),
        recopy: false,
    }
}
fn copy(comb: &Comb) -> CopiedEvidence {
    let target = comb.get(&"concepts/writer".parse().unwrap()).unwrap();
    serde_json::from_str(
        target.page.timeline[0]
            .text
            .strip_prefix("Source payload (JSON):\n\n    ")
            .unwrap(),
    )
    .unwrap()
}

fn assert_first_copy(comb: &Comb, original: &str) {
    assert_eq!(copy(comb).original_markdown.as_deref(), Some(original));
    assert_eq!(comb.stats().unwrap().inbox, 0);
    let target = comb.get(&"concepts/writer".parse().unwrap()).unwrap();
    assert_eq!(
        target.page.timeline.len(),
        1,
        "Nested source timeline stays opaque"
    );
    assert!(!target.page.metadata.owned().external());
}

fn interrupt_first_copy(comb: &mut Comb, note: &str, actor: &Actor) -> Consolidate {
    let first = request(comb, note);
    INTERRUPT_AFTER_TARGET.with(|flag| flag.set(true));
    assert!(
        comb.consolidate(
            &first,
            &fixture_write(actor, "interrupted", Some("curation-1")).unwrap()
        )
        .unwrap_err()
        .file_committed
    );
    assert_eq!(
        comb.stats().unwrap_err().code,
        crate::ErrorCode::RecoveryRequired
    );
    first
}

fn hand_edit(
    directory: &std::path::Path,
    comb: &mut Comb,
    slug: &str,
    edit: impl FnOnce(&mut Page),
) {
    let mut document = comb.get(&slug.parse().unwrap()).unwrap();
    edit(&mut document.page);
    std::fs::write(
        directory.join(format!("{slug}.md")),
        document.page.render().unwrap(),
    )
    .unwrap();
    comb.sync().unwrap();
}

fn assert_lost_coverage_requires_recopy(comb: &mut Comb, note: &str, actor: &Actor) {
    let mut next = request(comb, note);
    assert_eq!(
        comb.consolidate(
            &next,
            &fixture_write(actor, "retry-new-id", Some("curation-1")).unwrap()
        )
        .unwrap_err()
        .code,
        crate::ErrorCode::ConsolidationConflict
    );
    assert_eq!(
        comb.get(&"concepts/writer".parse().unwrap())
            .unwrap()
            .page
            .timeline
            .len(),
        1
    );
    next.recopy = true;
    comb.consolidate(
        &next,
        &fixture_write(actor, "explicit-recopy", Some("curation-1")).unwrap(),
    )
    .unwrap();
    assert!(copy(comb).recopy);
}

fn assert_retried_checkpoint(comb: &Comb, appended: bool) {
    let target = comb.get(&"concepts/writer".parse().unwrap()).unwrap();
    assert_eq!(target.page.timeline.len(), if appended { 2 } else { 1 });
    assert_eq!(
        target
            .page
            .timeline
            .last()
            .unwrap()
            .metadata
            .owned()
            .actor(),
        &Actor::External
    );
    if appended {
        let copied = copy(comb);
        assert!(copied.original_markdown.is_none());
        assert_eq!(copied.entries.len(), 1);
        assert_eq!(copied.entries[0].text, "Second measurement");
    }
    assert_eq!(comb.stats().unwrap().inbox, 0);
}

fn assert_recorded_recopy(comb: &mut Comb, note: &str, actor: &Actor, mut next: Consolidate) {
    next.recopy = true;
    let expected = comb.get(&note.parse().unwrap()).unwrap().markdown;
    comb.consolidate(
        &next,
        &fixture_write(actor, "recopy", Some("curation-1")).unwrap(),
    )
    .unwrap();
    assert!(copy(comb).recopy);
    assert_eq!(
        copy(comb).original_markdown.as_deref(),
        Some(expected.as_str())
    );
    assert!(
        comb.get(&"concepts/writer".parse().unwrap())
            .unwrap()
            .page
            .timeline[0]
            .metadata
            .owned()
            .consolidation()
            .unwrap()
            .recopy
    );
    next.recopy = false;
    assert_eq!(
        comb.consolidate(
            &next,
            &fixture_write(actor, "recopy", Some("curation-1")).unwrap()
        )
        .unwrap_err()
        .code,
        crate::ErrorCode::RequestConflict
    );
}

fn assert_edit_reopens_proposal(comb: &mut Comb, actor: &Actor) {
    comb.append(
        &"review/duplicate".parse().unwrap(),
        "New observation",
        "test:later",
        None,
        None,
        &fixture_write(actor, "reopen", Some("curation-1")).unwrap(),
    )
    .unwrap();
    assert_eq!(comb.stats().unwrap().unresolved_proposals, 1);
    assert_eq!(
        comb.get(&"review/duplicate".parse().unwrap())
            .unwrap()
            .page
            .timeline
            .len(),
        2
    );
}

mod cases_behaviour_1;
mod cases_learning_1;
mod cases_presentation_1;
mod cases_recovery_1;
mod cases_streaming_1;
