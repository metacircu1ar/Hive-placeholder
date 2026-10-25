use super::*;
use hive_setup_support::{
    files,
    migration::{Engine, Step},
};
use std::{fs, io, os::unix::fs::PermissionsExt, path::PathBuf};

struct Synthetic(u32, u32);
fn sqlite(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::other(error)
}
fn verify(root: &Path, number: u32) -> io::Result<()> {
    let copied = Copy::open(&root.join("runtime.sqlite"))?;
    let db = rusqlite::Connection::open(copied.path()).map_err(sqlite)?;
    let value: u32 = db
        .query_row("SELECT revision FROM authority", [], |r| r.get(0))
        .map_err(sqlite)?;
    if value != number
        || fs::read(root.join("format"))? != format!("{{\"format\":{number}}}").as_bytes()
    {
        return Err(io::Error::other("Synthetic database differs"));
    }
    drop(db);
    copied.check_source()
}
impl Step for Synthetic {
    fn from(&self) -> u32 {
        self.0
    }
    fn to(&self) -> u32 {
        self.1
    }
    fn files(&self, _: &Path) -> io::Result<Vec<PathBuf>> {
        Ok([
            "format",
            "runtime.sqlite",
            "runtime.sqlite-wal",
            "runtime.sqlite-shm",
            "runtime.sqlite-journal",
        ]
        .map(PathBuf::from)
        .to_vec())
    }
    fn databases(&self) -> Vec<PathBuf> {
        vec![PathBuf::from("runtime.sqlite")]
    }
    fn validate_before(&self, root: &Path) -> io::Result<()> {
        verify(root, self.0)
    }
    fn validate_after(&self, root: &Path) -> io::Result<()> {
        verify(root, self.1)
    }
    fn vacuum(&self, source: &Path, destination: &Path) -> io::Result<()> {
        let db = rusqlite::Connection::open(source).map_err(sqlite)?;
        db.execute("VACUUM INTO ?1", [destination.to_str().unwrap()])
            .map_err(sqlite)?;
        let saved = rusqlite::Connection::open(destination).map_err(sqlite)?;
        let count: u32 = saved
            .query_row("SELECT revision FROM authority", [], |r| r.get(0))
            .map_err(sqlite)?;
        if count != self.0 {
            return Err(io::Error::other("Invalid SQLite backup"));
        }
        Ok(())
    }
    fn transform(&self, root: &Path) -> io::Result<()> {
        {
            let db = rusqlite::Connection::open(root.join("runtime.sqlite")).map_err(sqlite)?;
            db.execute("UPDATE authority SET revision=?1", [self.1])
                .map_err(sqlite)?;
        }
        files::write(
            &root.join("format"),
            format!("{{\"format\":{}}}", self.1).as_bytes(),
            true,
        )
    }
}

#[test]
fn sqlite_wal_is_vacuumed_before_transform_and_original_bytes_remain_in_backup() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = root.join("runtime.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE authority(revision INTEGER); INSERT INTO authority VALUES(1)").unwrap();
    db.set_db_config(
        rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
        true,
    )
    .unwrap();
    drop(db);
    assert!(root.join("runtime.sqlite-wal").exists());
    let main = fs::read(&path).unwrap();
    let wal = fs::read(root.join("runtime.sqlite-wal")).unwrap();
    files::write(&root.join("format"), b"{\"format\":1}", false).unwrap();
    let owner = || Ok(());
    let engine = Engine::new(&root, &root, Path::new("format"), &owner).unwrap();
    let receipt = engine
        .apply(&Synthetic(1, 2), &atomic_fs::DefaultBarriers)
        .unwrap();
    let backup = root.join("backups").join(&receipt.backup);
    assert_eq!(fs::read(backup.join("runtime.sqlite")).unwrap(), main);
    assert_eq!(fs::read(backup.join("runtime.sqlite-wal")).unwrap(), wal);
    let saved =
        rusqlite::Connection::open(backup.join(".database-backups/runtime.sqlite")).unwrap();
    assert_eq!(
        saved
            .query_row("SELECT revision FROM authority", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        1
    );
    verify(&root, 2).unwrap();
    engine
        .apply(&Synthetic(2, 3), &atomic_fs::DefaultBarriers)
        .unwrap();
    verify(&root, 3).unwrap();
}

#[test]
fn inspection_leaves_original_database_wal_and_shm_exactly_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = temp.path().join("runtime.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    schema::initialize(&db).unwrap();
    db.set_db_config(
        rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
        true,
    )
    .unwrap();
    drop(db);
    let paths: Vec<_> = ["runtime.sqlite", "runtime.sqlite-wal", "runtime.sqlite-shm"]
        .map(|name| temp.path().join(name))
        .to_vec();
    for path in &paths {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let before: Vec<_> = paths.iter().map(|p| fs::read(p).unwrap()).collect();
    inspect(&path).unwrap();
    for (path, bytes) in paths.iter().zip(before) {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}
