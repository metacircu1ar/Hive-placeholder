use super::*;
use crate::{
    Actor,
    service::{Operation, auth::Connection},
    test_support::write,
};
use std::{fs, os::unix::fs::MetadataExt as _};

struct Fixture {
    temp: tempfile::TempDir,
    root: PathBuf,
    bridge: PathBuf,
    receipt: PathBuf,
}
use std::path::PathBuf;
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let result = Self {
            root: base.join("root"),
            bridge: base.join("bridge.json"),
            receipt: base.join("purge.json"),
            temp,
        };
        crate::setup::run_offline(
            &result.root,
            None,
            crate::setup::Options {
                for_hive: true,
                bridge_file: Some(result.bridge.clone()),
                ..crate::setup::Options::default()
            },
        )
        .unwrap();
        result
    }
    fn plan(&self, remove_root: bool) -> Plan<Details> {
        prepare(
            &self.root,
            &Args {
                detach_comb: true,
                bridge_file: Some(self.bridge.clone()),
                purge_comb_root: remove_root.then(|| self.root.clone()),
                ..Args::default()
            },
        )
        .unwrap()
    }
}
#[test]
fn default_purge_retains_trusted_pages_index_and_other_credentials() {
    let f = Fixture::new();
    let old = Connection::read(&f.bridge).unwrap();
    let mut comb = Comb::open(&f.root).unwrap();
    let slug = "facts/keep".parse().unwrap();
    let markdown = "---\ntitle: Retained fact\ntype: note\ntags: []\naliases: []\n---\nEvidence stays trusted.";
    comb.put_markdown(
        &slug,
        markdown,
        None,
        &write(&Actor::Human, "purge-fact", None).unwrap(),
    )
    .unwrap();
    service::offline(
        &mut comb,
        &Operation::CredentialCreate {
            grant: service::auth::Grant::Client {
                name: "other".into(),
                reviewer: false,
            },
        },
        None,
    )
    .unwrap();
    let identity = comb.identity();
    drop(comb);
    let plan = f.plan(false);
    plan.save(&f.receipt).unwrap();
    apply(&f.receipt).unwrap();
    assert!(!f.bridge.exists());
    assert!(!f.root.join(".comb/serve.json").exists());
    let mut comb = Comb::open(&f.root).unwrap();
    assert_eq!(comb.identity(), identity);
    assert!(!comb.get(&slug).unwrap().unverified);
    let listed = service::offline(&mut comb, &Operation::CredentialList, None).unwrap();
    let listed = serde_json::to_string(&listed).unwrap();
    assert!(listed.contains("other"));
    assert!(!listed.contains(&old.credential.id.to_string()));
    drop(comb);
    // A detached root needs an explicit selection before setup manages it again.
    provision::select_none(&f.root).unwrap();
    crate::setup::run_offline(&f.root, None, crate::setup::Options::default()).unwrap();
    assert!(!Comb::open(&f.root).unwrap().get(&slug).unwrap().unverified);
}
#[test]
fn full_root_purge_holds_stable_scope_and_never_deletes_recreated_root() {
    let f = Fixture::new();
    let plan = f.plan(true);
    plan.save(&f.receipt).unwrap();
    let lock = hive_setup_support::locks::path(&f.root, "comb-root", false).unwrap();
    assert!(apply(&f.receipt).is_err());
    drop(lock);
    apply(&f.receipt).unwrap();
    assert!(!f.root.exists());
    let recreated = Comb::init(&f.root).unwrap();
    let id = recreated.identity();
    drop(recreated);
    assert!(apply(&f.receipt).is_err());
    assert_eq!(Comb::open(&f.root).unwrap().identity(), id);
}
#[test]
fn changed_bridge_after_revocation_is_retained_even_with_same_inode() {
    let f = Fixture::new();
    let mut plan = f.plan(false);
    plan.details.bridge.as_mut().unwrap().revoked = true;
    plan.save(&f.receipt).unwrap();
    let inode = fs::metadata(&f.bridge).unwrap().ino();
    fs::write(&f.bridge, b"new credential contents").unwrap();
    assert_eq!(fs::metadata(&f.bridge).unwrap().ino(), inode);
    assert!(apply(&f.receipt).is_err());
    assert!(f.bridge.exists());
}
#[test]
fn retry_refuses_new_root_wiring() {
    let f = Fixture::new();
    let mut plan = f.plan(false);
    plan.details.phase = Phase::Detached;
    plan.save(&f.receipt).unwrap();
    // A newly configured root is sufficient to refuse cleanup.
    assert!(apply(&f.receipt).is_err());
}
#[test]
fn absent_root_is_never_adopted() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("new");
    let plan = prepare(
        &root,
        &Args {
            detach_comb: true,
            purge_comb_root: Some(root.clone()),
            ..Args::default()
        },
    )
    .unwrap();
    let receipt = temp.path().join("purge.json");
    plan.save(&receipt).unwrap();
    drop(Comb::init(&root).unwrap());
    assert!(apply(&receipt).is_err());
    assert!(root.join(".comb/identity.json").exists());
}

#[test]
fn sticky_temporary_parent_allows_init_and_contention_keeps_busy_code() {
    let root = std::env::temp_dir().join(format!(
        "comb-maintenance-{}",
        crate::values::CombId::generate()
    ));
    let comb = Comb::init(&root).unwrap();
    assert_eq!(
        Comb::open(&root).err().unwrap().code,
        crate::ErrorCode::CombBusy
    );
    drop(comb);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pending_fresh_setup_refuses_before_bridge_revocation() {
    let f = Fixture::new();
    f.plan(false).save(&f.receipt).unwrap();
    fs::write(f.root.join(".comb-setup.json"), b"pending").unwrap();
    assert!(
        apply(&f.receipt)
            .err()
            .unwrap()
            .message
            .contains("Finish fresh-root setup")
    );
    assert!(f.bridge.exists());
    assert!(f.root.join(".comb/serve.json").exists());
}

#[test]
fn reserved_receipt_does_not_revoke_bridge_or_remove_files() {
    for name in [".hive-tmp-state-ABC123", ".comb-tmp-admin-ABC123"] {
        let f = Fixture::new();
        let plan = f.plan(false);
        let reserved = f.temp.path().join(name);
        files::write(&reserved, &files::encode(&plan).unwrap(), false).unwrap();
        let before = fs::read(&f.bridge).unwrap();
        let connection = Connection::read(&f.bridge).unwrap();
        assert!(apply(&reserved).is_err());
        assert_eq!(fs::read(&f.bridge).unwrap(), before);
        service::auth::inspect_setup_bridge(&f.root, &connection, false).unwrap();
        assert!(f.root.join(".comb/serve.json").exists());
    }
}
