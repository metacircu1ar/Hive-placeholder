use super::*;
use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Owned(Child);
impl Drop for Owned {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
            self.0.wait().unwrap();
        }
    }
}
pub(super) fn child(root: &Path, name: &str, step: usize) {
    let mut child = Owned(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "index::schema::tests::process::native_fixture",
                "--ignored",
            ])
            .env("COMB_SCHEMA_ROOT", root)
            .env("COMB_SCHEMA_NAME", name)
            .env("COMB_SCHEMA_STEP", step.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert_eq!(status.code(), Some(73), "{name}/{step}");
            return;
        }
        assert!(Instant::now() < deadline, "owned schema child timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "owned process fixture invoked by schema crash and lock tests"]
fn native_fixture() {
    let root = PathBuf::from(std::env::var_os("COMB_SCHEMA_ROOT").unwrap());
    let name = std::env::var("COMB_SCHEMA_NAME").unwrap();
    let step: usize = std::env::var("COMB_SCHEMA_STEP").unwrap().parse().unwrap();
    if let Some(database) = name.strip_prefix("journal-") {
        leave_hot_journal(&root, database);
    }
    if name == "lock" {
        let db = Connection::open(root.join("index.sqlite")).unwrap();
        db.busy_timeout(Duration::ZERO).unwrap();
        let error = db.execute_batch("BEGIN IMMEDIATE").unwrap_err();
        assert_eq!(
            error.sqlite_error_code(),
            Some(rusqlite::ErrorCode::DatabaseBusy)
        );
        std::process::exit(73);
    }
    hooks::with(
        move |path, at| {
            if path.file_name().unwrap() == name.as_str() && at == STEPS[step] {
                std::process::exit(73);
            }
            Ok(())
        },
        || {
            drop(crate::Comb::open(&root).unwrap());
        },
    );
    panic!("schema crash boundary not reached");
}

#[test]
fn death_at_both_schema_publications_resumes_population_without_partial_databases() {
    for name in NAMES {
        for position in 0..STEPS.len() {
            let directory = directory();
            drop(crate::disk::Disk::acquire(directory.path(), true).unwrap());
            child(directory.path(), name, position);
            let admin = directory.path().join(".comb");
            let path = admin.join(name);
            assert_eq!(path.exists(), position >= 4);
            no_sidecars(&path);
            let inode = if path.exists() {
                complete(&path, name);
                Some(fs::metadata(&path).unwrap().ino())
            } else {
                None
            };
            let preparations: Vec<_> = fs::read_dir(&admin)
                .unwrap()
                .map(|e| e.unwrap().path())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(hive_setup_support::names::comb_admin_preparation_name)
                })
                .collect();
            assert_eq!(preparations.len(), usize::from(position < 4));
            for path in &preparations {
                assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o600);
            }
            assert!(admin.join("index-population.json").exists());
            drop(crate::Comb::open(directory.path()).unwrap());
            assert!(!admin.join("index-population.json").exists());
            if let Some(inode) = inode {
                assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
            }
            for name in NAMES {
                complete(&admin.join(name), name);
            }
            assert!(preparations.iter().all(|p| !p.exists()));
        }
    }
}

fn leave_hot_journal(root: &Path, name: &str) -> ! {
    let path = root.join(name);
    let db = Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; BEGIN IMMEDIATE;")
        .unwrap();
    let mutation = if name == "index.sqlite" {
        "INSERT INTO embedding_space VALUES(1,'uncommitted','uncommitted','{}')"
    } else {
        "INSERT INTO vectors VALUES('uncommitted','uncommitted',X'01020304')"
    };
    db.execute_batch(mutation).unwrap();
    db.cache_flush().unwrap();
    assert!(fs::metadata(companion(&path, "-journal")).unwrap().len() > 512);
    std::process::exit(73);
}

#[test]
fn normal_open_recovers_valid_hot_journals_for_both_existing_databases() {
    for name in NAMES {
        let directory = directory();
        retry(directory.path());
        child(directory.path(), &format!("journal-{name}"), 0);
        let journal = companion(&directory.path().join(name), "-journal");
        assert!(journal.exists());
        let index = Index::open(&directory.path().join("index.sqlite")).unwrap();
        assert!(index.active_space().unwrap().is_none());
        assert_eq!(
            index
                .connection
                .query_row("SELECT count(*) FROM vector_cache.vectors", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(index);
        assert!(!journal.exists());
    }
}

#[test]
fn comb_open_recovers_hot_journals_before_automatic_schema_selection() {
    for name in NAMES {
        let root = directory();
        drop(crate::Comb::init(root.path()).unwrap());
        let admin = root.path().join(".comb");
        child(&admin, &format!("journal-{name}"), 0);
        let journal = companion(&admin.join(name), "-journal");
        assert!(journal.exists());
        let comb = crate::Comb::open(root.path()).unwrap();
        assert!(comb.index.active_space().unwrap().is_none());
        assert_eq!(comb.stats().unwrap().pages, 0);
        drop(comb);
        assert!(!journal.exists());
    }
}
