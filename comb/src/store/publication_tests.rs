use super::*;
use crate::test_support::{TestComb, write as fixture_write};
use crate::{ErrorCode, read::Snapshot};
use std::{fs, os::unix::fs::PermissionsExt};

const MARKDOWN: &str = "---\ntitle: Publication\n---\nOriginal content.";

fn setup() -> (tempfile::TempDir, Comb, Mutation) {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    let created = comb
        .put_markdown(
            &"concepts/publication".parse().unwrap(),
            MARKDOWN,
            None,
            &fixture_write(&Actor::Human, "create", None).unwrap(),
        )
        .unwrap();
    (root, comb, created)
}

fn reject_intent_directory(comb: &Comb) -> fs::Permissions {
    let prior = fs::metadata(&comb.disk.admin).unwrap().permissions();
    // Reads still work, but atomic intent replacement rejects shared write access
    // before creating an intent. This is deterministic even for privileged tests.
    fs::set_permissions(&comb.disk.admin, fs::Permissions::from_mode(0o720)).unwrap();
    prior
}

#[test]
fn uncommitted_persist_failure_restores_reads_and_allows_same_request_retry() {
    let (root, mut comb, created) = setup();
    let path = root.path().join("concepts/publication.md");
    let original = fs::read(&path).unwrap();
    let write = fixture_write(&Actor::Human, "append", None).unwrap();
    let prior = reject_intent_directory(&comb);
    let error = comb
        .append(&created.slug, "New evidence", "fixture", None, None, &write)
        .unwrap_err();
    fs::set_permissions(&comb.disk.admin, prior).unwrap();
    assert_eq!(error.code, ErrorCode::UnsafePath);
    assert_eq!(error.message, Error::unsafe_directory().message);
    assert!(!error.file_committed);
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(!comb.disk.admin.join("intent.json").exists());
    assert!(
        comb.disk
            .receipt(&write.principal, &write.request_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        comb.disk.head(&created.slug).unwrap(),
        Some(created.hash.clone())
    );
    comb.health.check().unwrap();
    comb.ready().unwrap();
    assert_eq!(
        *comb.publication.lock().unwrap(),
        crate::read::PublicationState::Ready
    );
    assert_eq!(comb.get(&created.slug).unwrap().hash, created.hash);
    let reader = Index::reader(&comb.disk.index_path(), None).unwrap();
    let snapshot = Snapshot::pin(reader, &comb.publication).unwrap();
    assert_eq!(
        snapshot.index.document(&created.slug).unwrap().hash,
        created.hash
    );
    drop(snapshot);
    let retried = comb
        .append(&created.slug, "New evidence", "fixture", None, None, &write)
        .unwrap();
    assert!(!retried.replayed);
    assert_ne!(retried.hash, created.hash);
    assert!(
        comb.append(&created.slug, "New evidence", "fixture", None, None, &write)
            .unwrap()
            .replayed
    );
}

#[test]
fn uncommitted_remove_failure_keeps_reads_fenced_until_reopen() {
    let (root, mut comb, created) = setup();
    let path = root.path().join("concepts/publication.md");
    let original = fs::read(&path).unwrap();
    let write = fixture_write(&Actor::Human, "remove", None).unwrap();
    let prior = reject_intent_directory(&comb);
    let error = comb
        .remove(&created.slug, &created.hash, &write)
        .unwrap_err();
    fs::set_permissions(&comb.disk.admin, prior).unwrap();
    assert_eq!(error.code, ErrorCode::UnsafePath);
    assert_eq!(error.message, Error::unsafe_directory().message);
    assert!(!error.file_committed);
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(!comb.disk.admin.join("intent.json").exists());
    assert!(
        comb.disk
            .receipt(&write.principal, &write.request_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        comb.disk.head(&created.slug).unwrap(),
        Some(created.hash.clone())
    );
    comb.health.check().unwrap();
    assert_eq!(comb.ready().unwrap_err().code, ErrorCode::RecoveryRequired);
    assert_eq!(
        *comb.publication.lock().unwrap(),
        crate::read::PublicationState::Pending
    );
    assert_eq!(
        comb.get(&created.slug).unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    let reader = Index::reader(&comb.disk.index_path(), None).unwrap();
    assert_eq!(
        Snapshot::pin(reader, &comb.publication).err().unwrap().code,
        ErrorCode::RecoveryRequired
    );
    drop(comb);
    let mut reopened = Comb::open(root.path()).unwrap();
    assert_eq!(reopened.get(&created.slug).unwrap().hash, created.hash);
    assert!(
        !reopened
            .remove(&created.slug, &created.hash, &write)
            .unwrap()
            .replayed
    );
}
