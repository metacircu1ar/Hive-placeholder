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

fn child(root: &Path, name: &str, position: usize, publication: bool, occurrence: usize) {
    let mut child = Owned(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "disk::rebuilding::tests::process::crash_fixture",
                "--ignored",
            ])
            .env("COMB_REBUILD_ROOT", root)
            .env("COMB_REBUILD_NAME", name)
            .env("COMB_REBUILD_POSITION", position.to_string())
            .env("COMB_REBUILD_PUBLICATION", publication.to_string())
            .env("COMB_REBUILD_OCCURRENCE", occurrence.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert_eq!(status.code(), Some(73), "{name}/{position}");
            return;
        }
        assert!(Instant::now() < deadline, "owned rebuild child timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "owned crash fixture invoked by rebuild recovery tests"]
fn crash_fixture() {
    let root = std::path::PathBuf::from(std::env::var_os("COMB_REBUILD_ROOT").unwrap());
    let name = std::env::var("COMB_REBUILD_NAME").unwrap();
    let position: usize = std::env::var("COMB_REBUILD_POSITION")
        .unwrap()
        .parse()
        .unwrap();
    if let Some(mode) = name.strip_prefix("native-") {
        leave_native_state(&root, mode, NAMES[position]);
    }
    let publication: bool = std::env::var("COMB_REBUILD_PUBLICATION")
        .unwrap()
        .parse()
        .unwrap();
    let at = if publication {
        PUBLICATION[position]
    } else {
        DELETION[position]
    };
    let occurrence: usize = std::env::var("COMB_REBUILD_OCCURRENCE")
        .unwrap()
        .parse()
        .unwrap();
    let count = std::cell::Cell::new(0);
    hooks::with(
        move |path, step| {
            if path.file_name().unwrap() == name.as_str() && step == at {
                count.set(count.get() + 1);
                if count.get() == occurrence {
                    std::process::exit(73);
                }
            }
            Ok(())
        },
        || drop(Comb::rebuild(&root).unwrap()),
    );
    panic!("rebuild death boundary was not reached");
}

fn leave_native_state(root: &Path, mode: &str, name: &str) -> ! {
    assert!(matches!(mode, "DELETE" | "WAL"));
    let path = root.join(".comb").join(name);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.pragma_update(None, "journal_mode", mode).unwrap();
    db.execute_batch("PRAGMA synchronous=FULL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    let main = name == "index.sqlite";
    let table = if main { "embedding_space" } else { "vectors" };
    let committed = if main {
        "INSERT INTO embedding_space VALUES(1,'committed','committed','{}')"
    } else {
        "INSERT INTO vectors VALUES('committed','committed',X'01020304')"
    };
    db.execute_batch(committed).unwrap();
    db.execute_batch(&format!("BEGIN IMMEDIATE; DELETE FROM {table};"))
        .unwrap();
    db.cache_flush().unwrap();
    let suffix = if mode == "WAL" { "-wal" } else { "-journal" };
    assert!(
        fs::metadata(path.with_file_name(format!("{name}{suffix}")))
            .unwrap()
            .len()
            > 512
    );
    std::process::exit(73);
}

#[test]
fn rebuild_recovers_native_vector_journal_and_wal_and_discards_native_text_state() {
    use std::os::unix::fs::MetadataExt;
    for mode in ["DELETE", "WAL"] {
        for position in [0, 4] {
            let root = fixture(false);
            child(root.path(), &format!("native-{mode}"), position, false, 1);
            let admin = root.path().join(".comb");
            let cache = admin.join("vectors.sqlite");
            let inode = fs::metadata(&cache).unwrap().ino();
            drop(Comb::rebuild(root.path()).unwrap());
            assert_eq!(fs::metadata(&cache).unwrap().ino(), inode);
            let db = rusqlite::Connection::open(&cache).unwrap();
            let count: i64 = db
                .query_row("SELECT count(*) FROM vectors", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, i64::from(position == 4));
            drop(db);
            let db = rusqlite::Connection::open(admin.join("index.sqlite")).unwrap();
            let count: i64 = db
                .query_row("SELECT count(*) FROM embedding_space", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0);
            drop(db);
            recover(root.path());
        }
    }
}

#[test]
fn death_during_selected_deletions_and_marker_publication_resumes_on_open() {
    for name in NAMES {
        for position in 0..DELETION.len() {
            let root = fixture(true);
            child(root.path(), name, position, false, 1);
            recover(root.path());
        }
    }
    for name in [INTENT, POPULATION] {
        for position in 0..PUBLICATION.len() {
            let root = fixture(true);
            child(root.path(), name, position, true, 1);
            if name == INTENT && position < 4 {
                drop(Comb::rebuild(root.path()).unwrap());
            }
            recover(root.path());
        }
    }
}

#[test]
fn death_during_repeat_record_sync_and_final_handoff_resumes_the_accepted_generation() {
    for name in [INTENT, POPULATION] {
        for position in [1, 2, 5, 6] {
            let root = fixture(true);
            child(root.path(), name, position, true, 2);
            recover(root.path());
        }
    }
    for position in 0..DELETION.len() {
        let root = fixture(true);
        child(
            root.path(),
            INTENT,
            position,
            false,
            if position < 2 { 1 } else { 3 },
        );
        recover(root.path());
    }
    for position in [1, 2] {
        let root = fixture(true);
        child(root.path(), POPULATION, position, true, 3);
        assert!(!root.path().join(".comb").join(INTENT).exists());
        recover(root.path());
    }
}
