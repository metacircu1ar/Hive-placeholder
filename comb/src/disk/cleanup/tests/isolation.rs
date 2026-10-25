use super::*;
use crate::service::auth::{Connection, ConnectionExt, CredentialOutput};
use std::{sync::mpsc, time::Duration};

#[test]
fn active_and_completed_credentials_and_same_shaped_attachments_survive() {
    let root = fixture();
    let admin = root.path().join(".comb");
    let output = admin.join("export.json");
    let prepared = CredentialOutput::prepare(&output).unwrap();
    let stages: Vec<_> = fs::read_dir(&admin)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".comb-tmp-")
        })
        .collect();
    assert_eq!(stages.len(), 1);
    let attachments = root.path().join("attachments");
    fs::create_dir(&attachments).unwrap();
    candidate(&attachments.join(SHORT));
    candidate(&admin.join(LONG));
    drop(Comb::open(root.path()).unwrap());
    assert!(stages[0].exists());
    assert!(!admin.join(LONG).exists());
    let connection = Connection::human(root.path(), admin.join("service.sock")).unwrap();
    prepared.publish(&connection).unwrap();
    let original = fs::read(&output).unwrap();
    drop(Comb::open(root.path()).unwrap());
    assert_eq!(fs::read(&output).unwrap(), original);
    assert!(attachments.join(SHORT).exists());
}

#[test]
fn live_nested_root_page_preparation_is_never_a_parent_cleanup_candidate() {
    let root = fixture();
    let nested = root.path().join(".comb/heads");
    let mut writer = Comb::init(&nested).unwrap();
    let target = writer.disk.root.join("page.md");
    let (arrived, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        hooks::with(
            move |path, step| {
                if path == target && step == Step::Written {
                    arrived.send(()).unwrap();
                    released.recv_timeout(Duration::from_secs(10)).unwrap();
                }
                Ok(())
            },
            || {
                writer
                    .put_markdown(
                        &"page".parse().unwrap(),
                        "---\ntitle: Nested\n---\nKeep",
                        None,
                        &crate::test_support::write(&crate::Actor::Human, "nested", None).unwrap(),
                    )
                    .unwrap()
            },
        );
    });
    waiting.recv_timeout(Duration::from_secs(10)).unwrap();
    let stage = fs::read_dir(nested.join(".comb"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".comb-tmp-")
        })
        .unwrap();
    assert!(hive_setup_support::names::comb_admin_preparation_name(
        stage.file_name().unwrap()
    ));
    candidate(&root.path().join(".comb").join(LONG));
    drop(Comb::open(root.path()).unwrap());
    assert!(stage.exists());
    release.send(()).unwrap();
    thread.join().unwrap();
    assert!(nested.join("page.md").exists());
    assert!(
        Comb::open(&nested)
            .unwrap()
            .get(&"page".parse().unwrap())
            .is_ok()
    );
}

#[test]
fn sqlite_alias_refusal_preserves_the_live_transaction_lock() {
    let root = tempfile::tempdir().unwrap();
    let disk = Disk::acquire(root.path(), true).unwrap();
    let database = disk.index_path();
    atomic(&database, b"", &System).unwrap();
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection.execute_batch("PRAGMA journal_mode=DELETE; CREATE TABLE data(value); BEGIN IMMEDIATE; INSERT INTO data VALUES(1)").unwrap();
    processes::probe(root.path(), true);
    let path = disk.admin.join(SHORT);
    candidate(&path);
    let target = path.clone();
    let original = database.clone();
    let error = hooks::with(
        move |at, step| {
            if at == target && step == Step::BeforeRemove {
                fs::rename(&target, target.with_extension("kept"))?;
                fs::hard_link(&original, &target)?;
            }
            Ok(())
        },
        || disk.cleanup_preparations(),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::UnsafePath);
    assert!(path.exists());
    processes::probe(root.path(), true);
    connection.execute_batch("ROLLBACK").unwrap();
    processes::probe(root.path(), false);
}
