use super::*;
use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
            self.0.wait().unwrap();
        }
    }
}
fn spawn(root: &Path, mode: &str, target: &str) -> Process {
    Process(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "disk::cleanup::tests::processes::process_fixture",
                "--ignored",
                "--nocapture",
            ])
            .env("COMB_SWEEP_ROOT", root)
            .env("COMB_SWEEP_MODE", mode)
            .env("COMB_SWEEP_TARGET", target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    )
}
fn wait(child: &mut Process) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "owned cleanup subprocess timed out"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
pub(super) fn probe(root: &Path, busy: bool) {
    let mut child = spawn(root, "probe", if busy { "busy" } else { "free" });
    assert!(wait(&mut child).success());
}

fn preparations(root: &Path) -> Vec<PathBuf> {
    [".comb", ".comb/heads", ".comb/receipts"]
        .into_iter()
        .flat_map(|name| {
            fs::read_dir(root.join(name))
                .unwrap()
                .map(|entry| entry.unwrap().path())
        })
        .filter(|path| {
            hive_setup_support::names::comb_admin_preparation_name(path.file_name().unwrap())
        })
        .collect()
}

#[test]
fn actual_publishers_exclude_cleanup_until_owned_process_death() {
    for target in [
        "intent.json",
        "heads",
        "receipts",
        "index-population.json",
        "index.sqlite",
        "index-rebuild.json",
        "credentials.json",
        "import.json",
    ] {
        let root = fixture();
        let mut child = spawn(root.path(), "publisher", target);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !root.path().join(".ready").exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "publisher exited before {target}"
            );
            assert!(
                Instant::now() < deadline,
                "publisher did not reach {target}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let pending = preparations(root.path());
        assert_eq!(pending.len(), 1, "{target}");
        assert_eq!(
            Comb::open(root.path()).err().unwrap().code,
            ErrorCode::CombBusy
        );
        assert!(pending[0].exists());
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        drop(child);
        // Exercise the other supported suffix beside the actual native stage.
        candidate(&root.path().join(".comb").join(SHORT));
        drop(Comb::open(root.path()).unwrap());
        assert!(preparations(root.path()).is_empty(), "{target}");
    }
}

#[test]
fn owned_death_at_cleanup_boundaries_allows_reopening() {
    for target in ["before-remove", "after-remove", "before-sync", "after-sync"] {
        let root = fixture();
        candidate(&root.path().join(".comb").join(LONG));
        let mut child = spawn(root.path(), "cleanup", target);
        assert_eq!(wait(&mut child).code(), Some(73));
        let remained = root.path().join(".comb").join(LONG).exists();
        assert_eq!(remained, target == "before-remove");
        drop(Comb::open(root.path()).unwrap());
        assert!(preparations(root.path()).is_empty());
    }
}

#[test]
#[ignore = "owned publisher/death/SQLite probe fixture invoked by cleanup parent tests"]
fn process_fixture() {
    let root = PathBuf::from(std::env::var_os("COMB_SWEEP_ROOT").unwrap());
    let target = std::env::var("COMB_SWEEP_TARGET").unwrap();
    match std::env::var("COMB_SWEEP_MODE").unwrap().as_str() {
        "probe" => {
            let connection = rusqlite::Connection::open(root.join(".comb/index.sqlite")).unwrap();
            connection.busy_timeout(Duration::ZERO).unwrap();
            let result = connection.execute_batch("BEGIN IMMEDIATE");
            if target == "busy" {
                assert_eq!(
                    result.unwrap_err().sqlite_error_code(),
                    Some(rusqlite::ErrorCode::DatabaseBusy)
                );
            } else {
                result.unwrap();
                connection.execute_batch("ROLLBACK").unwrap();
            }
        }
        "cleanup" => {
            let canonical = root.canonicalize().unwrap();
            let step = match target.as_str() {
                "before-remove" => Step::BeforeRemove,
                "after-remove" => Step::AfterRemove,
                "before-sync" => Step::BeforeDirectorySync,
                _ => Step::AfterDirectorySync,
            };
            hooks::with(
                move |path, at| {
                    if at == step
                        && (path == canonical.join(".comb")
                            || path == canonical.join(".comb").join(LONG))
                    {
                        std::process::exit(73);
                    }
                    Ok(())
                },
                || {
                    drop(Comb::open(&root).unwrap());
                },
            );
            panic!("cleanup death boundary not reached");
        }
        "publisher" => publisher(&root, &target),
        _ => panic!("unknown owned fixture mode"),
    }
}

fn publisher(root: &Path, target: &str) {
    let mut comb = Comb::open(root).unwrap();
    let ready = root.join(".ready");
    let expected = target.to_owned();
    let input = root.join(".import-source");
    fs::create_dir(&input).unwrap();
    atomic(
        &input.join("imported.md"),
        b"---\ntitle: Import\n---\nContent",
        &System,
    )
    .unwrap();
    hooks::with(
        move |path, step| {
            if step == Step::Written
                && (path.file_name().unwrap() == expected.as_str()
                    || path.parent().unwrap().file_name().unwrap() == expected.as_str())
            {
                fs::write(&ready, b"ready")?;
                std::thread::sleep(Duration::from_secs(10));
                std::process::exit(75);
            }
            Ok(())
        },
        || match target {
            "index.sqlite" | "index-rebuild.json" => {
                drop(comb);
                drop(Comb::rebuild(root).unwrap());
            }
            "index-population.json" => {
                comb.disk.index_population(true).unwrap();
            }
            "import.json" => {
                comb.import(&input, None).unwrap();
            }
            "credentials.json" => {
                use crate::service::auth::{Grant, Registry};
                let registry = Registry::open(&comb.disk, comb.health.clone()).unwrap();
                registry
                    .create(Grant::Client {
                        name: "test".into(),
                        reviewer: false,
                    })
                    .unwrap();
            }
            _ => {
                comb.put_markdown(
                    &"kept".parse().unwrap(),
                    "---\ntitle: Kept\n---\nContent",
                    None,
                    &crate::test_support::write(&crate::Actor::Human, "create", None).unwrap(),
                )
                .unwrap();
            }
        },
    );
    panic!("publication boundary not reached: {target}");
}
