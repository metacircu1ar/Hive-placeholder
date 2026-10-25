use super::*;
use crate::state::{State, directory};

#[test]
fn copied_database_and_wal_match_without_modifying_source_files() {
    let root = tempfile::tempdir().unwrap();
    let old = State::new(root.path().join("old")).unwrap();
    let (path, _db) = super::super::tests::wal_export_fixture(&old);
    let paths = [
        path.clone(),
        path.with_file_name("runtime.sqlite-wal"),
        path.with_file_name("runtime.sqlite-shm"),
    ];
    let before: Vec<_> = paths.iter().map(|path| fs::read(path).unwrap()).collect();
    let snapshot = snapshot_database(&path).unwrap();
    for (source, bytes) in paths.iter().zip(&before) {
        assert_eq!(fs::read(source).unwrap(), *bytes);
    }
    for name in ["runtime.sqlite", "runtime.sqlite-wal"] {
        let target = snapshot.path().join(name);
        assert_eq!(
            target.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::read(target).unwrap(),
            fs::read(path.with_file_name(name)).unwrap()
        );
    }
    assert!(!snapshot.path().join("runtime.sqlite-shm").exists());
}

#[test]
fn source_change_after_copy_is_rejected_and_disposable_snapshot_removed() {
    let root = tempfile::tempdir().unwrap();
    let runtime = root.path().join("runtime");
    directory(&runtime).unwrap();
    let source = runtime.join("runtime.sqlite");
    crate::state::private_file(&source, true).unwrap();
    fs::write(&source, b"before").unwrap();
    let snapshot = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let target = snapshot.path().to_owned();
    let result = crate::publication::prepare(snapshot, |snapshot| {
        copy_snapshot(&source, snapshot, || {
            fs::write(&source, b"after").map_err(Error::state_io_from)
        })
    });
    assert_eq!(result.unwrap_err().code(), "recovery_required");
    assert!(!target.exists());
    assert_eq!(fs::read(source).unwrap(), b"after");
}
