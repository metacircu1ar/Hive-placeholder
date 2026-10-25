use super::*;
use crate::{Comb, ErrorCode};
use std::{
    fs,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::atomic::Ordering,
};

impl Registry {
    pub(crate) fn poison_records_for_test(&self) {
        assert!(
            catch_unwind(AssertUnwindSafe(|| {
                let _records = self.records.write().unwrap();
                panic!("injected registry writer panic");
            }))
            .is_err()
        );
    }
}

fn setup() -> (tempfile::TempDir, Comb, Registry) {
    let root = tempfile::tempdir().unwrap();
    let comb = Comb::init(root.path()).unwrap();
    let registry = Registry::open(&comb.disk, comb.health.clone()).unwrap();
    (root, comb, registry)
}

fn human(root: &Path) -> Credential {
    Connection::human(root, root.join(".comb/comb.sock"))
        .unwrap()
        .credential
}

fn client(name: &str) -> Grant {
    Grant::Client {
        name: name.into(),
        reviewer: false,
    }
}

fn assert_registry_fenced(registry: &Registry, human: &Credential, client: &Credential) {
    for credential in [human, client] {
        let error = registry.authorize(credential, None).err().unwrap();
        assert_eq!(error.code, ErrorCode::RecoveryRequired);
        assert!(!error.file_committed);
        assert_eq!(error.clone().code, ErrorCode::RecoveryRequired);
    }
    assert_eq!(
        registry.list().err().unwrap().code,
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        registry
            .create(super::Grant::Client {
                name: "Blocked".into(),
                reviewer: false
            })
            .err()
            .unwrap()
            .code,
        ErrorCode::RecoveryRequired
    );
    assert_eq!(
        registry.revoke(&client.id).unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
}

#[test]
fn registry_write_panic_fences_human_clients_and_comb_until_reopen() {
    let (root, mut comb, registry) = setup();
    let credential = registry.create(client("Writer")).unwrap();
    let human = human(root.path());
    let (_other_root, other_comb, other_registry) = setup();
    let other_credential = other_registry.create(client("Independent")).unwrap();
    registry.poison_records_for_test();
    // Guard unwinding publishes the fence; no subsequent acquisition is needed.
    assert_eq!(
        comb.health.check().unwrap_err().code,
        ErrorCode::RecoveryRequired
    );
    assert_registry_fenced(&registry, &human, &credential);
    assert_eq!(
        comb.index().err().unwrap().code,
        ErrorCode::RecoveryRequired
    );
    let write = crate::store::Write::new(&Actor::Human, &"blocked".parse().unwrap(), None).unwrap();
    assert_eq!(
        comb.capture("Blocked observation", &write)
            .unwrap_err()
            .code,
        ErrorCode::RecoveryRequired
    );
    other_comb.health.check().unwrap();
    assert!(other_registry.authorize(&other_credential, None).is_ok());
    assert_eq!(other_registry.list().unwrap().len(), 1);
    drop(comb);
    let reopened = Comb::open(root.path()).unwrap();
    let restored = Registry::open(&reopened.disk, reopened.health.clone()).unwrap();
    assert!(restored.authorize(&human, None).unwrap().actor().is_human());
    assert!(restored.authorize(&credential, None).is_ok());
    assert_registry_fenced(&registry, &human, &credential);
}

#[test]
fn registry_reader_unwind_does_not_poison_healthy_records() {
    let (root, comb, registry) = setup();
    let credential = registry.create(client("Writer")).unwrap();
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _records = registry.records.read().unwrap();
            panic!("injected registry reader panic");
        }))
        .is_err()
    );
    comb.health.check().unwrap();
    assert!(
        registry
            .authorize(&human(root.path()), None)
            .unwrap()
            .actor()
            .is_human()
    );
    assert!(registry.authorize(&credential, None).is_ok());
    registry.revoke(&credential.id).unwrap();
    assert!(registry.list().unwrap().is_empty());
}

#[test]
fn post_atomic_health_failure_reports_committed_create_and_revoke() {
    for revoke in [false, true] {
        let (root, comb, registry) = setup();
        let credential = registry.create(client("Existing")).unwrap();
        registry.fence_after_atomic.store(true, Ordering::SeqCst);
        let error = if revoke {
            registry.revoke(&credential.id).unwrap_err()
        } else {
            registry.create(client("Durable new grant")).err().unwrap()
        };
        assert_eq!(error.code, ErrorCode::RecoveryRequired);
        assert!(error.file_committed);
        assert!(!registry.broken.load(Ordering::SeqCst));
        assert_registry_fenced(&registry, &human(root.path()), &credential);
        let raw: serde_json::Value =
            serde_json::from_slice(&fs::read(&registry.path).unwrap()).unwrap();
        let expected = if revoke { 0 } else { 2 };
        assert_eq!(raw["credentials"].as_array().unwrap().len(), expected);
        drop(comb);
        let reopened = Comb::open(root.path()).unwrap();
        let restored = Registry::open(&reopened.disk, reopened.health.clone()).unwrap();
        assert_eq!(restored.list().unwrap().len(), expected);
        assert_eq!(restored.authorize(&credential, None).is_ok(), !revoke);
        assert!(
            restored
                .authorize(&human(root.path()), None)
                .unwrap()
                .actor()
                .is_human()
        );
    }
}

#[test]
fn ordinary_registry_write_uncertainty_keeps_its_separate_policy() {
    let (root, comb, registry) = setup();
    let credential = registry.create(client("Existing")).unwrap();
    let saved = registry.path.with_extension("saved");
    fs::rename(&registry.path, &saved).unwrap();
    fs::create_dir(&registry.path).unwrap();
    assert!(registry.create(client("Cannot persist")).is_err());
    assert!(registry.broken.load(Ordering::SeqCst));
    comb.health.check().unwrap();
    assert!(
        registry
            .authorize(&human(root.path()), None)
            .unwrap()
            .actor()
            .is_human()
    );
    assert_eq!(
        registry.authorize(&credential, None).err().unwrap().code,
        ErrorCode::Forbidden
    );
    assert_eq!(registry.list().unwrap().len(), 1);
    assert_eq!(
        registry
            .create(client("Still uncertain"))
            .err()
            .unwrap()
            .message,
        Error::credential_registry_uncertain().message
    );
    fs::remove_dir(&registry.path).unwrap();
    fs::rename(saved, &registry.path).unwrap();
}

#[test]
fn registry_flush_uses_the_disk_owner_and_preserves_uncertainty_until_reopen() {
    let root = tempfile::tempdir().unwrap();
    let mut comb = Comb::init(root.path()).unwrap();
    let durability = Arc::new(disk::Recording::default());
    comb.disk.set_durability(durability.clone());
    let registry = Registry::open(&comb.disk, comb.health.clone()).unwrap();
    let existing = registry.create(client("Existing")).unwrap();
    durability.reset();
    durability.barrier(Ok(()));
    durability.fail(disk::Operation::FullFlush, 1);
    let error = registry
        .create(client("Committed but unacknowledged"))
        .err()
        .unwrap();
    assert_eq!(error.code, ErrorCode::IoError);
    assert_eq!(
        error.message,
        Error::from(std::io::Error::from_raw_os_error(libc::EIO)).message
    );
    assert!(error.file_committed);
    assert!(error.request_id.is_none());
    assert_eq!(durability.counts().full_barriers, 1);
    assert!(registry.broken.load(Ordering::SeqCst));
    assert_eq!(registry.list().unwrap().len(), 1);
    assert_eq!(
        registry.authorize(&existing, None).err().unwrap().code,
        ErrorCode::Forbidden
    );
    comb.health.check().unwrap();
    assert!(
        registry
            .authorize(&human(root.path()), None)
            .unwrap()
            .actor()
            .is_human()
    );
    let reopened = Registry::open(&comb.disk, comb.health.clone()).unwrap();
    assert_eq!(reopened.list().unwrap().len(), 2);
    assert!(reopened.authorize(&existing, None).is_ok());
}
