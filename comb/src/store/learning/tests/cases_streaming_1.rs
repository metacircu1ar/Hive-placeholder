//! Streaming regressions.
use super::*;

#[test]
fn source_change_after_target_commit_returns_committed_conflict() {
    let (directory, mut comb, note) = setup();
    let actor = agent("fixture", "curator", Some("curator"), "execution", "turn");
    let request = request(&comb, &note);
    let path = directory.path().join(format!("{}.md", request.note));
    comb.before_index = Some(Box::new(move || {
        std::fs::write(
            path,
            "---\ntitle: Changed source\n---\nNew external evidence.",
        )
        .unwrap();
    }));
    let write = fixture_write(&actor, "target-then-conflict", Some("curation-1")).unwrap();
    let error = comb.consolidate(&request, &write).unwrap_err();
    assert_eq!(error.code, crate::ErrorCode::RecoveryRequired);
    assert!(error.file_committed);
    assert!(
        comb.disk
            .receipt(&write.principal, &write.request_id)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        comb.ready().unwrap_err().code,
        crate::ErrorCode::RecoveryRequired
    );
    let edited = std::fs::read(directory.path().join(format!("{}.md", request.note))).unwrap();
    drop(comb);
    let error = Comb::open(directory.path()).err().unwrap();
    assert!(error.file_committed);
    assert_eq!(error.code, crate::ErrorCode::RecoveryRequired);
    assert_eq!(
        std::fs::read(directory.path().join(format!("{}.md", request.note))).unwrap(),
        edited
    );
    assert!(directory.path().join(".comb/intent.json").exists());
}
