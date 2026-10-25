use super::*;
use crate::ErrorCode;
use crate::test_support::{TestComb, assert_code, write as fixture_write};
use std::panic::{AssertUnwindSafe, catch_unwind};

#[test]
fn publication_unwind_fences_the_comb_until_reopen() {
    let (root, comb) = TestComb::new().unwrap().into_parts();
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _guard = comb.publication.lock().unwrap();
            panic!("publication fixture");
        }))
        .is_err()
    );
    assert_eq!(
        comb.index().err().unwrap().code,
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        comb.get(&"concepts/a".parse().unwrap()).unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    drop(comb);
    Comb::open(root.path()).unwrap().ready().unwrap();
}

#[test]
fn poison_after_file_write_reports_commit_and_preserves_recovery_intent() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    let health = comb.health.clone();
    comb.before_index = Some(Box::new(move || health.fence()));
    let slug = "concepts/a".parse().unwrap();
    let error = assert_code!(
        comb.put_markdown(
            &slug,
            "---\ntitle: A\n---\nFact",
            None,
            &fixture_write(&Actor::Human, "create", None).unwrap()
        ),
        ErrorCode::RecoveryRequired
    );
    assert!(error.file_committed);
    assert!(root.path().join(".comb/intent.json").exists());
    assert!(root.path().join("concepts/a.md").exists());
    assert!(comb.ready().is_err());
    drop(comb);
    let comb = Comb::open(root.path()).unwrap();
    assert_eq!(comb.index().unwrap().stats().unwrap().pages, 1);
    assert!(comb.get(&slug).unwrap().markdown.contains("Fact"));
    assert!(!root.path().join(".comb/intent.json").exists());
}

#[test]
fn poison_after_unlink_reports_commit_and_preserves_removal_receipt() {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    let slug = "concepts/a".parse().unwrap();
    let created = comb
        .put_markdown(
            &slug,
            "---\ntitle: A\n---\nFact",
            None,
            &fixture_write(&Actor::Human, "create", None).unwrap(),
        )
        .unwrap();
    let health = comb.health.clone();
    comb.before_index = Some(Box::new(move || health.fence()));
    let error = assert_code!(
        comb.remove(
            &slug,
            &created.hash,
            &fixture_write(&Actor::Human, "remove", None).unwrap()
        ),
        ErrorCode::RecoveryRequired
    );
    assert!(error.file_committed);
    assert!(root.path().join(".comb/intent.json").exists());
    assert!(!root.path().join("concepts/a.md").exists());
    assert!(
        comb.replay_remove(
            &slug,
            &created.hash,
            &fixture_write(&Actor::Human, "remove", None).unwrap()
        )
        .is_err()
    );
    drop(comb);
    let mut comb = Comb::open(root.path()).unwrap();
    assert!(
        comb.remove(
            &slug,
            &created.hash,
            &fixture_write(&Actor::Human, "remove", None).unwrap()
        )
        .unwrap()
        .replayed
    );
    assert_eq!(comb.index().unwrap().stats().unwrap().pages, 0);
    assert!(!root.path().join(".comb/intent.json").exists());
}
