use super::*;
use crate::runtime::store::{Queries, Store};
use std::{cell::RefCell, fs, os::unix::fs::MetadataExt, path::PathBuf};
mod concurrency;
mod process;

const STEPS: [Step; 7] = [
    Step::Written,
    Step::BeforeFileSync,
    Step::AfterFileSync,
    Step::BeforePublish,
    Step::AfterPublish,
    Step::BeforeDirectorySync,
    Step::AfterDirectorySync,
];
type Hook = Box<dyn Fn(Step) -> io::Result<()>>;
thread_local! { static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) }; }
pub(super) fn step(value: Step) -> io::Result<()> {
    HOOK.with_borrow(|hook| hook.as_ref().map_or(Ok(()), |hook| hook(value)))
}
struct Restore(Option<Hook>);
impl Drop for Restore {
    fn drop(&mut self) {
        HOOK.replace(self.0.take());
    }
}
fn with_hook<T>(hook: impl Fn(Step) -> io::Result<()> + 'static, run: impl FnOnce() -> T) -> T {
    let _restore = Restore(HOOK.replace(Some(Box::new(hook))));
    run()
}
fn publication(error: &Error) -> &atomic_fs::Error {
    use std::error::Error as _;
    error
        .source()
        .unwrap()
        .downcast_ref::<io::Error>()
        .unwrap()
        .get_ref()
        .unwrap()
        .downcast_ref::<atomic_fs::Error>()
        .unwrap()
}
fn complete(path: &Path) {
    assert!(fs::metadata(path).unwrap().len() > 0);
    assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o600);
    let connection =
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    schema::validate(&connection).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}
fn no_sidecars(path: &Path) {
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut companion = path.as_os_str().to_os_string();
        companion.push(suffix);
        assert!(!Path::new(&companion).exists());
    }
}
fn retry(path: &Path) {
    let existing = path.exists();
    let started = std::time::Instant::now();
    let store = Store::open(path).unwrap();
    eprintln!(
        "database_open existing={existing} elapsed_us={} bytes={}",
        started.elapsed().as_micros(),
        fs::metadata(path).unwrap().len()
    );
    assert_eq!(
        store
            .connection
            .query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "wal"
    );
    for (name, expected) in [
        ("synchronous", 2),
        ("fullfsync", 0),
        ("foreign_keys", 1),
        ("busy_timeout", 0),
    ] {
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, name, |r| r.get::<_, i64>(0))
                .unwrap(),
            expected
        );
    }
    drop(store);
    complete(path);
}

#[test]
fn publication_failures_leave_absence_or_a_complete_database_and_preserve_error_evidence() {
    for (index, at) in STEPS.into_iter().enumerate() {
        let temporary = crate::runtime::test_directory();
        let path = temporary.path().join("runtime.sqlite");
        let error = with_hook(
            move |step| {
                if step == at {
                    Err(io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
            || Store::open(&path),
        )
        .err()
        .unwrap();
        assert_eq!(error.code(), "state_io");
        let failure = publication(&error);
        assert_eq!(failure.published(), index >= 4);
        assert_eq!(failure.io_error().raw_os_error(), Some(libc::EIO));
        assert!(failure.cleanup_error().is_none());
        assert!(failure.retained().is_none());
        assert_eq!(path.exists(), index >= 4);
        no_sidecars(&path);
        if path.exists() {
            complete(&path);
        }
        assert_eq!(
            fs::read_dir(temporary.path()).unwrap().count(),
            usize::from(index >= 4)
        );
        retry(&path);
    }
}

#[test]
fn reopen_repeats_parent_sync_before_admitting_a_published_database() {
    let temporary = crate::runtime::test_directory();
    let path = temporary.path().join("runtime.sqlite");
    for iteration in 0..2 {
        let error = with_hook(
            |step| {
                if step == Step::BeforeDirectorySync {
                    Err(io::Error::from_raw_os_error(libc::EIO))
                } else {
                    Ok(())
                }
            },
            || Store::open(&path),
        )
        .err()
        .unwrap();
        if iteration == 0 {
            assert!(publication(&error).published());
        } else {
            assert_eq!(error.io_raw_os_error(), Some(libc::EIO));
        }
        complete(&path);
        no_sidecars(&path);
        assert!(!temporary.path().join("output").exists());
    }
    retry(&path);
}

#[test]
fn existing_empty_databases_are_rejected_without_rewriting_or_creating_companions() {
    for header in [false, true] {
        let temporary = crate::runtime::test_directory();
        let path = temporary.path().join("runtime.sqlite");
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .create_new(&path, b"")
            .unwrap();
        if header {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch("CREATE TABLE scratch(x); DROP TABLE scratch;")
                .unwrap();
        }
        let before = fs::read(&path).unwrap();
        for error in [
            Store::open(&path).err().unwrap(),
            super::super::check_existing_format(&path).unwrap_err(),
            Queries::open_read_only(&path).err().unwrap(),
        ] {
            assert_eq!(error.code(), "unsupported_schema");
        }
        assert_eq!(fs::read(&path).unwrap(), before);
        no_sidecars(&path);
        assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 1);
    }
}

#[test]
fn missing_file_during_publication_or_sqlite_admission_is_never_recreated() {
    for already_published in [false, true] {
        let temporary = crate::runtime::test_directory();
        let path = temporary.path().join("runtime.sqlite");
        if already_published {
            // Reach SQLite admission's directory sync independently of the
            // publication helper's stronger final identity check.
            publish(&path, &Publication).unwrap();
        }
        let removed = path.clone();
        let error = with_hook(
            move |step| {
                if step == Step::AfterDirectorySync {
                    fs::remove_file(&removed)?;
                }
                Ok(())
            },
            || Store::open(&path),
        )
        .err()
        .unwrap();
        if already_published {
            assert_eq!(error.code(), "database_error");
        } else {
            assert_eq!(error.code(), "state_io");
            let failure = publication(&error);
            assert!(failure.published());
            assert_eq!(failure.io_error().raw_os_error(), Some(libc::ENOENT));
        }
        assert!(!path.exists());
        no_sidecars(&path);
    }
}

#[test]
fn native_schema_allocation_failure_leaves_no_partial_schema_transaction() {
    let memory = Connection::open_in_memory().unwrap();
    memory.execute_batch("PRAGMA max_page_count=1").unwrap();
    let error = schema::initialize(&memory).unwrap_err();
    assert_eq!(
        crate::runtime::store::sqlite_code(&error),
        rusqlite::ffi::SQLITE_FULL
    );
    assert!(memory.is_autocommit());
    assert!(schema::empty(&memory).unwrap());
}
