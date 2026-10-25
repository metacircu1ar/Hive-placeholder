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
fn crash(root: &Path, source: &Path, mode: &str, step: usize, id: &ImportId) {
    let mut child = Owned(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "import::tests::process::crash_fixture",
                "--ignored",
            ])
            .env("COMB_IMPORT_ROOT", root)
            .env("COMB_IMPORT_SOURCE", source)
            .env("COMB_IMPORT_MODE", mode)
            .env("COMB_IMPORT_STEP", step.to_string())
            .env("COMB_IMPORT_ID", id.as_str())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert_eq!(status.code(), Some(73), "{mode}/{step}");
            return;
        }
        assert!(Instant::now() < deadline, "owned import child timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "owned crash fixture invoked by import recovery tests"]
fn crash_fixture() {
    let root = PathBuf::from(std::env::var_os("COMB_IMPORT_ROOT").unwrap());
    let source = PathBuf::from(std::env::var_os("COMB_IMPORT_SOURCE").unwrap());
    let mode = std::env::var("COMB_IMPORT_MODE").unwrap();
    let position: usize = std::env::var("COMB_IMPORT_STEP").unwrap().parse().unwrap();
    let id: ImportId = std::env::var("COMB_IMPORT_ID").unwrap().parse().unwrap();
    let mut comb = Comb::open(&root).unwrap();
    let occurrence = if mode == "complete" { 2 } else { 1 };
    let count = Cell::new(0);
    hooks::with(
        move |path, at| {
            if path.file_name().unwrap() == "import.json" && at == Step::Written {
                count.set(count.get() + 1);
            }
            if path.file_name().unwrap() == "import.json"
                && count.get() == occurrence
                && at == STEPS[position]
            {
                std::process::exit(73);
            }
            Ok(())
        },
        || {
            if mode == "abandon" {
                comb.abandon_import(&id).unwrap();
            } else {
                comb.import(&source, None).unwrap();
            }
        },
    );
    panic!("import death boundary was not reached");
}

#[test]
fn death_during_batch_accept_complete_or_abandon_preserves_recovery() {
    for mode in ["accept", "complete", "abandon"] {
        for position in 0..STEPS.len() {
            let mut fixture = Fixture::new();
            let id = if mode == "abandon" {
                fixture.pending(1).batch_id
            } else {
                ImportId::generate()
            };
            // Drop only the writer while retaining both owned directories.
            let Fixture { comb, root, source } = fixture;
            drop(comb);
            crash(root.path(), source.path(), mode, position, &id);
            let mut child_fixture = Fixture {
                comb: Comb::open(root.path()).unwrap(),
                root,
                source,
            };
            if mode == "abandon" {
                fs::remove_dir_all(child_fixture.source.path()).unwrap();
                let report = child_fixture.comb.abandon_import(&id).unwrap();
                assert_eq!((report.completed, report.unfinished), (1, 2));
                assert_eq!(report.state, ImportState::Abandoned);
            } else {
                let before = child_fixture.comb.import_status().unwrap().current;
                assert_eq!(before.is_some(), mode == "complete" || position >= 4);
                let report = child_fixture.apply().unwrap();
                if let Some(before) = before {
                    assert_eq!(before.batch_id, report.batch_id);
                }
                assert_eq!(report.total, 3);
            }
        }
    }
}
