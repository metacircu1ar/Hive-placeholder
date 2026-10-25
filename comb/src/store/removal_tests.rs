use super::*;
use crate::ErrorCode;
use crate::test_support::{TestComb, write as fixture_write};
use std::{fs, path::PathBuf};

const MARKDOWN: &str = "---\ntitle: Original\n---\nOriginal evidence.";

fn setup() -> (tempfile::TempDir, Comb, Mutation) {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    let created = comb
        .put_markdown(
            &"concepts/remove".parse().unwrap(),
            MARKDOWN,
            None,
            &fixture_write(&Actor::Human, "create", None).unwrap(),
        )
        .unwrap();
    (root, comb, created)
}

fn receipt_file(root: &Path, request_id: &str) -> PathBuf {
    fs::read_dir(root.join(".comb/receipts"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            let record: serde_json::Value =
                serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            record["request_id"] == request_id
        })
        .unwrap()
}

fn assert_fenced(comb: &mut Comb, created: &Mutation) {
    assert_eq!(
        comb.replay_remove(
            &created.slug,
            &created.hash,
            &fixture_write(&Actor::Human, "remove", None).unwrap()
        )
        .unwrap_err()
        .code,
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        comb.remove(
            &created.slug,
            &created.hash,
            &fixture_write(&Actor::Human, "remove", None).unwrap()
        )
        .unwrap_err()
        .code,
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        comb.index().err().unwrap().code,
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        *comb.publication.lock().unwrap(),
        crate::read::PublicationState::Pending
    );
}

#[test]
fn removal_replays_after_reopen_and_new_requests_keep_missing_page_errors() {
    let (root, mut comb, created) = setup();
    let removed = comb
        .remove(
            &created.slug,
            &created.hash,
            &fixture_write(&Actor::Human, "remove", None).unwrap(),
        )
        .unwrap();
    assert_eq!(removed.removed, created.slug);
    assert_eq!(removed.request_id.as_str(), "remove");
    assert!(!removed.replayed);
    assert!(!root.path().join("concepts/remove.md").exists());
    assert_eq!(comb.index().unwrap().stats().unwrap().pages, 0);
    assert!(
        comb.remove(
            &created.slug,
            &created.hash,
            &fixture_write(&Actor::Human, "remove", None).unwrap()
        )
        .unwrap()
        .replayed
    );
    assert_missing_results_keep_receipt_errors(&mut comb, &created);
    drop(comb);
    let mut comb = Comb::open(root.path()).unwrap();
    assert!(
        comb.remove(
            &created.slug,
            &created.hash,
            &fixture_write(&Actor::Human, "remove", None).unwrap()
        )
        .unwrap()
        .replayed
    );
    assert!(!root.path().join(".comb/intent.json").exists());
}

fn assert_missing_results_keep_receipt_errors(comb: &mut Comb, created: &Mutation) {
    assert_eq!(
        comb.remove(
            &created.slug,
            &created.hash,
            &fixture_write(&Actor::Human, "fresh", None).unwrap()
        )
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );
    // Page-producing receipts retain their existing missing-result behavior.
    assert_eq!(
        comb.put_markdown(
            &created.slug,
            MARKDOWN,
            None,
            &fixture_write(&Actor::Human, "create", None).unwrap()
        )
        .unwrap_err()
        .code,
        ErrorCode::ResultRemoved
    );
}

#[test]
fn removal_replay_leaves_valid_and_malformed_recreations_untouched() {
    let (root, mut comb, created) = setup();
    comb.remove(
        &created.slug,
        &created.hash,
        &fixture_write(&Actor::Human, "remove", None).unwrap(),
    )
    .unwrap();
    let replacement = comb
        .put_markdown(
            &created.slug,
            "---\ntitle: Recreated\n---\nReplacement.",
            None,
            &fixture_write(&Actor::Human, "recreate", None).unwrap(),
        )
        .unwrap();
    let path = root.path().join("concepts/remove.md");
    let saved = fs::read(&path).unwrap();
    let head = comb.disk.head(&created.slug).unwrap();
    let indexed = comb.index.content_hash(&created.slug).unwrap();
    assert_valid_recreation_survives(&mut comb, &created, &replacement, &path, &saved);
    fs::write(&path, b"malformed replacement without frontmatter").unwrap();
    let malformed = fs::read(&path).unwrap();
    assert_both_remove_entry_points_replay(&mut comb, &created);
    assert_eq!(fs::read(&path).unwrap(), malformed);
    assert_eq!(comb.disk.head(&created.slug).unwrap(), head);
    assert_eq!(comb.index.content_hash(&created.slug).unwrap(), indexed);
    assert!(!root.path().join(".comb/intent.json").exists());
    drop(comb);
    let mut comb = Comb::open(root.path()).unwrap();
    assert!(
        comb.remove(
            &created.slug,
            &created.hash,
            &fixture_write(&Actor::Human, "remove", None).unwrap()
        )
        .unwrap()
        .replayed
    );
    assert_eq!(fs::read(&path).unwrap(), malformed);
}

fn assert_both_remove_entry_points_replay(comb: &mut Comb, created: &Mutation) {
    assert!(
        comb.replay_remove(
            &created.slug,
            &created.hash,
            &fixture_write(&Actor::Human, "remove", None).unwrap()
        )
        .unwrap()
        .unwrap()
        .replayed
    );
    assert!(
        comb.remove(
            &created.slug,
            &created.hash,
            &fixture_write(&Actor::Human, "remove", None).unwrap()
        )
        .unwrap()
        .replayed
    );
}

fn assert_valid_recreation_survives(
    comb: &mut Comb,
    created: &Mutation,
    replacement: &Mutation,
    path: &std::path::Path,
    saved: &[u8],
) {
    assert!(
        comb.remove(
            &created.slug,
            &created.hash,
            &fixture_write(&Actor::Human, "remove", None).unwrap()
        )
        .unwrap()
        .replayed
    );
    assert_eq!(fs::read(path).unwrap(), saved);
    assert_eq!(
        comb.remove(
            &created.slug,
            &created.hash,
            &fixture_write(&Actor::Human, "fresh", None).unwrap()
        )
        .unwrap_err()
        .code,
        ErrorCode::Conflict
    );
    assert_eq!(comb.get(&created.slug).unwrap().hash, replacement.hash);
}

#[test]
fn removal_ids_conflict_across_arguments_and_operations_before_target_access() {
    let (root, mut comb, created) = setup();
    assert_eq!(
        comb.remove(
            &created.slug,
            &created.hash,
            &fixture_write(&Actor::Human, "create", None).unwrap()
        )
        .unwrap_err()
        .code,
        ErrorCode::RequestConflict
    );
    comb.remove(
        &created.slug,
        &created.hash,
        &fixture_write(&Actor::Human, "remove", None).unwrap(),
    )
    .unwrap();
    fs::write(root.path().join("concepts/remove.md"), b"not a page").unwrap();
    for (slug, hash) in [
        (created.slug.clone(), ContentHash::digest("different hash")),
        ("concepts/absent".parse().unwrap(), created.hash.clone()),
    ] {
        assert_eq!(
            comb.remove(
                &slug,
                &hash,
                &fixture_write(&Actor::Human, "remove", None).unwrap()
            )
            .unwrap_err()
            .code,
            ErrorCode::RequestConflict
        );
    }
    assert_eq!(
        comb.put_markdown(
            &created.slug,
            "invalid markdown",
            None,
            &fixture_write(&Actor::Human, "remove", None).unwrap()
        )
        .unwrap_err()
        .code,
        ErrorCode::RequestConflict
    );
    assert_eq!(
        fs::read(root.path().join("concepts/remove.md")).unwrap(),
        b"not a page"
    );
    assert_remove_replay_ignores_run(&mut comb, &created);
}

fn assert_remove_replay_ignores_run(comb: &mut Comb, created: &Mutation) {
    // The run attribution is irrelevant to the removal payload.
    let changed_run = Write::new(
        &Actor::Human,
        &"remove".parse().unwrap(),
        Some("different-run"),
    )
    .unwrap();
    assert!(
        comb.remove(&created.slug, &created.hash, &changed_run)
            .unwrap()
            .replayed
    );
}

#[test]
fn removal_requires_human_authority_before_receipt_access() {
    let (root, mut comb, created) = setup();
    let receipts = root.path().join(".comb/receipts");
    let saved = root.path().join(".comb/saved-receipts");
    fs::rename(&receipts, &saved).unwrap();
    fs::write(&receipts, b"receipt access must fail if attempted").unwrap();
    for actor in [
        Actor::Client {
            id: "client".into(),
            name: "Client".into(),
        },
        Actor::Collector {
            bridge: "key".into(),
        },
        Actor::Agent {
            bridge: "key".into(),
            id: "agent".into(),
            role: None,
            execution: "execution".into(),
            turn: "turn".into(),
        },
    ] {
        let write = Write::new(&actor, &"remove".parse().unwrap(), None).unwrap();
        assert_eq!(
            comb.replay_remove(&created.slug, &created.hash, &write)
                .unwrap_err()
                .code,
            ErrorCode::Forbidden
        );
        assert_eq!(
            comb.remove(&created.slug, &created.hash, &write)
                .unwrap_err()
                .code,
            ErrorCode::Forbidden
        );
    }
    assert_eq!(comb.get(&created.slug).unwrap().hash, created.hash);
    assert!(!root.path().join(".comb/intent.json").exists());
    fs::remove_file(&receipts).unwrap();
    fs::rename(saved, receipts).unwrap();
}

#[test]
fn removal_keeps_operation_claims_opaque_but_rejects_a_wrong_receipt_slug() {
    let (root, mut comb, created) = setup();
    comb.remove(
        &created.slug,
        &created.hash,
        &fixture_write(&Actor::Human, "remove", None).unwrap(),
    )
    .unwrap();
    let path = receipt_file(root.path(), "remove");
    let mut receipt: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for operation in ["", "opaque operation claim"] {
        receipt["operation"] = serde_json::json!(operation);
        let bytes = serde_json::to_vec(&receipt).unwrap();
        fs::write(&path, &bytes).unwrap();
        assert!(
            comb.remove(
                &created.slug,
                &created.hash,
                &fixture_write(&Actor::Human, "remove", None).unwrap()
            )
            .unwrap()
            .replayed
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    receipt["slug"] = serde_json::json!("concepts/another");
    fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
    let error = comb
        .remove(
            &created.slug,
            &created.hash,
            &fixture_write(&Actor::Human, "remove", None).unwrap(),
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidState);
    assert_eq!(error.message, Error::receipt_slug_mismatch().message);
}

#[test]
fn failed_finalization_receipt_stays_fenced_until_recovery() {
    let (root, mut comb, created) = setup();
    let durability = std::sync::Arc::new(crate::disk::Recording::default());
    comb.disk.set_durability(durability.clone());
    durability.reset();
    durability.fail_finish_once();
    let error = comb
        .remove(
            &created.slug,
            &created.hash,
            &fixture_write(&Actor::Human, "remove", None).unwrap(),
        )
        .unwrap_err();
    assert!(error.file_committed);
    assert!(
        comb.disk
            .receipt(
                &Actor::Human.principal().unwrap(),
                &"remove".parse().unwrap()
            )
            .unwrap()
            .is_some()
    );
    assert_fenced(&mut comb, &created);
    assert!(!root.path().join("concepts/remove.md").exists());
    drop(comb);
    let mut comb = Comb::open(root.path()).unwrap();
    assert!(
        comb.remove(
            &created.slug,
            &created.hash,
            &fixture_write(&Actor::Human, "remove", None).unwrap()
        )
        .unwrap()
        .replayed
    );
    assert!(!root.path().join(".comb/intent.json").exists());
    assert_eq!(comb.index().unwrap().stats().unwrap().pages, 0);
}

#[test]
fn recovery_completes_unlinked_removal_and_recreates_missing_sidecars() {
    let (root, mut comb, created) = setup();
    comb.index.reject_page_delete().unwrap();
    let error = comb
        .remove(
            &created.slug,
            &created.hash,
            &fixture_write(&Actor::Human, "remove", None).unwrap(),
        )
        .unwrap_err();
    assert!(error.file_committed);
    assert_fenced(&mut comb, &created);
    assert!(!root.path().join("concepts/remove.md").exists());
    assert_eq!(comb.index.stats().unwrap().pages, 1);
    comb.index.allow_page_delete().unwrap();
    fs::remove_file(receipt_file(root.path(), "remove")).unwrap();
    for head in fs::read_dir(root.path().join(".comb/heads")).unwrap() {
        fs::remove_file(head.unwrap().path()).unwrap();
    }
    drop(comb);
    let mut comb = Comb::open(root.path()).unwrap();
    assert!(
        comb.remove(
            &created.slug,
            &created.hash,
            &fixture_write(&Actor::Human, "remove", None).unwrap()
        )
        .unwrap()
        .replayed
    );
    assert_eq!(comb.disk.head(&created.slug).unwrap(), None);
    assert_eq!(comb.index().unwrap().stats().unwrap().pages, 0);
    assert!(receipt_file(root.path(), "remove").exists());
}

#[test]
fn recovery_retracts_removal_receipt_when_original_bytes_survive() {
    let (root, mut comb, created) = setup();
    let path = root.path().join("concepts/remove.md");
    let original = fs::read(&path).unwrap();
    comb.index.reject_page_delete().unwrap();
    assert!(
        comb.remove(
            &created.slug,
            &created.hash,
            &fixture_write(&Actor::Human, "remove", None).unwrap()
        )
        .unwrap_err()
        .file_committed
    );
    comb.index.allow_page_delete().unwrap();
    crate::disk::atomic(&path, &original, &crate::disk::System).unwrap();
    drop(comb);
    let mut comb = Comb::open(root.path()).unwrap();
    assert_eq!(comb.get(&created.slug).unwrap().hash, created.hash);
    assert_eq!(
        comb.disk.head(&created.slug).unwrap(),
        Some(created.hash.clone())
    );
    assert!(
        comb.disk
            .receipt(
                &Actor::Human.principal().unwrap(),
                &"remove".parse().unwrap()
            )
            .unwrap()
            .is_none()
    );
    assert!(
        !comb
            .remove(
                &created.slug,
                &created.hash,
                &fixture_write(&Actor::Human, "remove", None).unwrap()
            )
            .unwrap()
            .replayed
    );
}

#[test]
fn pending_removal_with_unexpected_content_fails_closed() {
    let (root, mut comb, created) = setup();
    comb.index.reject_page_delete().unwrap();
    assert!(
        comb.remove(
            &created.slug,
            &created.hash,
            &fixture_write(&Actor::Human, "remove", None).unwrap()
        )
        .unwrap_err()
        .file_committed
    );
    comb.index.allow_page_delete().unwrap();
    let path = root.path().join("concepts/remove.md");
    fs::write(&path, b"unexpected replacement").unwrap();
    drop(comb);
    assert_eq!(
        Comb::open(root.path()).err().unwrap().code,
        ErrorCode::RecoveryRequired
    );
    assert_eq!(fs::read(&path).unwrap(), b"unexpected replacement");
    assert!(root.path().join(".comb/intent.json").exists());
}

#[test]
fn removal_preserves_precommit_fencing_and_both_device_barriers() {
    let (root, mut comb, created) = setup();
    let durability = std::sync::Arc::new(crate::disk::Recording::default());
    comb.disk.set_durability(durability.clone());
    durability.reset();
    comb.remove(
        &created.slug,
        &created.hash,
        &fixture_write(&Actor::Human, "remove", None).unwrap(),
    )
    .unwrap();
    assert_eq!(
        durability.counts().full_barriers,
        2 + durability.counts().order_fallbacks
    );
    let recreated = comb
        .put_markdown(
            &created.slug,
            MARKDOWN,
            None,
            &fixture_write(&Actor::Human, "recreate", None).unwrap(),
        )
        .unwrap();
    fs::write(root.path().join(".comb/intent.json"), b"pending state").unwrap();
    let error = comb
        .remove(
            &created.slug,
            &recreated.hash,
            &fixture_write(&Actor::Human, "fresh", None).unwrap(),
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::RecoveryRequired);
    assert!(!error.file_committed);
    assert_fenced(&mut comb, &created);
    assert_eq!(
        ContentHash::digest(fs::read(root.path().join("concepts/remove.md")).unwrap()),
        recreated.hash
    );
}
