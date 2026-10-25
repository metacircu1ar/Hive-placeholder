use super::*;
use crate::test_support::{TestComb, write as fixture_write};
use crate::{ErrorCode, disk::publication_hooks as hooks, read::Snapshot};
use atomic_fs::Step;
use std::fs;

const ORIGINAL: &str = "---\ntitle: Original\n---\nOriginal summary.";
const EDITED: &str = "---\ntitle: Editor\n---\nIndependent editor update.";
#[derive(Clone, Copy)]
enum Operation {
    Put,
    Append,
    Remove,
    Reconcile,
    Create,
}
impl Operation {
    fn slug(self, created: &Mutation) -> Slug {
        if matches!(self, Self::Create) {
            "concepts/new".parse().unwrap()
        } else {
            created.slug.clone()
        }
    }
    fn run(self, comb: &mut Comb, created: &Mutation) -> Result<()> {
        let slug = self.slug(created);
        let write = fixture_write(&Actor::Human, "attempt", None)?;
        match self {
            Self::Put | Self::Create => comb
                .put_markdown(
                    &slug,
                    "---\ntitle: Application\n---\nApplication update.",
                    (!matches!(self, Self::Create)).then_some(&created.hash),
                    &write,
                )
                .map(|_| ()),
            Self::Append => comb
                .append(&slug, "New fact", "fixture", None, None, &write)
                .map(|_| ()),
            Self::Remove => comb.remove(&slug, &created.hash, &write).map(|_| ()),
            Self::Reconcile => comb.sync().map(|_| ()),
        }
    }
}
fn fixture(operation: Operation) -> (tempfile::TempDir, Comb, Mutation, std::path::PathBuf) {
    let (root, mut comb) = TestComb::new().unwrap().into_parts();
    let created = comb
        .put_markdown(
            &"concepts/page".parse().unwrap(),
            ORIGINAL,
            None,
            &fixture_write(&Actor::Human, "seed", None).unwrap(),
        )
        .unwrap();
    let path = comb
        .disk
        .root
        .join(format!("{}.md", operation.slug(&created)));
    if matches!(operation, Operation::Reconcile) {
        fs::write(
            &path,
            "---\ntitle: First edit\n---\nFirst independent edit.",
        )
        .unwrap();
    }
    (root, comb, created, path)
}
fn receipt_absent(comb: &Comb) {
    let write = fixture_write(&Actor::Human, "attempt", None).unwrap();
    assert!(
        comb.disk
            .receipt(&write.principal, &write.request_id)
            .unwrap()
            .is_none()
    );
}
fn independent_reader(comb: &Comb) -> Result<Snapshot> {
    Snapshot::pin(
        Index::reader(&comb.disk.index_path(), None)?,
        &comb.publication,
    )
}

#[test]
fn changed_loaded_baseline_refuses_before_intent_and_keeps_readers_usable() {
    for operation in [
        Operation::Put,
        Operation::Append,
        Operation::Remove,
        Operation::Reconcile,
        Operation::Create,
    ] {
        let (_root, mut comb, created, path) = fixture(operation);
        let edited = path.clone();
        let error = hooks::with_checkpoint(
            move |at, step| {
                if at == edited && step == hive_setup_support::checkpoints::Checkpoint::BeforeIntent
                {
                    fs::write(at, EDITED)?;
                }
                Ok(())
            },
            || operation.run(&mut comb, &created),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::Conflict);
        assert!(!error.file_committed);
        assert_eq!(fs::read_to_string(&path).unwrap(), EDITED);
        assert!(!comb.disk.admin.join("intent.json").exists());
        receipt_absent(&comb);
        comb.ready().unwrap();
        let snapshot = independent_reader(&comb).unwrap();
        assert_eq!(snapshot.index.stats().unwrap().pages, 1);
        assert_eq!(
            snapshot.index.document(&created.slug).unwrap().hash,
            created.hash
        );
    }
}

#[test]
fn changed_target_after_intent_is_preserved_and_fences_recovery() {
    for operation in [
        Operation::Put,
        Operation::Append,
        Operation::Remove,
        Operation::Reconcile,
        Operation::Create,
    ] {
        let (root, mut comb, created, path) = fixture(operation);
        let edited = path.clone();
        let boundary = if matches!(operation, Operation::Remove) {
            Step::BeforeRemove
        } else {
            Step::BeforePublish
        };
        let error = hooks::with_checkpoint(
            move |at, step| {
                if at == edited && step == boundary.into() {
                    fs::write(at, EDITED)?;
                }
                Ok(())
            },
            || operation.run(&mut comb, &created),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::RecoveryRequired);
        assert!(error.file_committed);
        assert_eq!(fs::read_to_string(&path).unwrap(), EDITED);
        assert!(comb.disk.admin.join("intent.json").exists());
        receipt_absent(&comb);
        assert_eq!(comb.ready().unwrap_err().code, ErrorCode::RecoveryRequired);
        assert_eq!(
            independent_reader(&comb).err().unwrap().code,
            ErrorCode::RecoveryRequired
        );
        drop(comb);
        assert!(Comb::open(root.path()).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), EDITED);
        assert!(root.path().join(".comb/intent.json").exists());
    }
}

#[test]
fn pre_intent_native_removal_failure_still_fences_the_handle() {
    let (root, mut comb, created, path) = fixture(Operation::Remove);
    let selected = path.clone();
    let before = fs::read(&path).unwrap();
    let error = hooks::with_checkpoint(
        move |at, step| {
            if at == selected && step == hive_setup_support::checkpoints::Checkpoint::BeforeIntent {
                return Err(std::io::Error::from_raw_os_error(libc::EIO));
            }
            Ok(())
        },
        || Operation::Remove.run(&mut comb, &created),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::IoError);
    assert!(!error.file_committed);
    assert_eq!(comb.ready().unwrap_err().code, ErrorCode::RecoveryRequired);
    assert!(independent_reader(&comb).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(!comb.disk.admin.join("intent.json").exists());
    drop(comb);
    Comb::open(root.path()).unwrap().ready().unwrap();
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn creator_during_intent_publication_keeps_winner_and_requires_recovery() {
    use std::error::Error as _;
    let (root, mut comb, created, path) = fixture(Operation::Create);
    let intent = comb.disk.admin.join("intent.json");
    let winner = path.clone();
    let error = hooks::with_checkpoint(
        move |at, step| {
            if at == intent
                && step
                    == hive_setup_support::checkpoints::Checkpoint::Publication(Step::AfterPublish)
            {
                fs::write(&winner, EDITED)?;
            }
            Ok(())
        },
        || Operation::Create.run(&mut comb, &created),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::RecoveryRequired);
    assert!(error.file_committed);
    let native = error
        .source()
        .unwrap()
        .downcast_ref::<atomic_fs::Error>()
        .unwrap();
    assert_eq!(native.io_error().kind(), std::io::ErrorKind::AlreadyExists);
    assert!(!native.published());
    assert_eq!(fs::read_to_string(&path).unwrap(), EDITED);
    assert!(comb.disk.admin.join("intent.json").exists());
    receipt_absent(&comb);
    assert_eq!(comb.ready().unwrap_err().code, ErrorCode::RecoveryRequired);
    assert!(independent_reader(&comb).is_err());
    drop(comb);
    assert!(Comb::open(root.path()).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), EDITED);
    assert!(root.path().join(".comb/intent.json").exists());
}
