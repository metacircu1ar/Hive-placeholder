use super::*;
use crate::disk::durable::{Durability, System};
use std::{
    fs::File,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const FINISH: [Step; 6] = [
    Step::BeforeFileSync,
    Step::AfterFileSync,
    Step::BeforeRemove,
    Step::AfterRemove,
    Step::BeforeDirectorySync,
    Step::AfterDirectorySync,
];

struct Owned(Child);
impl Drop for Owned {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
            self.0.wait().unwrap();
        }
    }
}

fn crash(root: &Path, request: &Consolidate, path: &Path, slot: usize, position: usize) {
    let mut child = Owned(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "disk::journal::consolidation::tests::process::crash_fixture",
                "--ignored",
            ])
            .env("COMB_PAIR_ROOT", root)
            .env("COMB_PAIR_REQUEST", serde_json::to_string(request).unwrap())
            .env("COMB_PAIR_PATH", path)
            .env("COMB_PAIR_SLOT", slot.to_string())
            .env("COMB_PAIR_POSITION", position.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert_eq!(status.code(), Some(73), "{slot}/{position}");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "owned consolidation child timed out"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "owned crash fixture invoked by consolidation recovery tests"]
fn crash_fixture() {
    let root = PathBuf::from(std::env::var_os("COMB_PAIR_ROOT").unwrap());
    let path = PathBuf::from(std::env::var_os("COMB_PAIR_PATH").unwrap());
    let request: Consolidate =
        serde_json::from_str(&std::env::var("COMB_PAIR_REQUEST").unwrap()).unwrap();
    let slot: usize = std::env::var("COMB_PAIR_SLOT").unwrap().parse().unwrap();
    let position: usize = std::env::var("COMB_PAIR_POSITION")
        .unwrap()
        .parse()
        .unwrap();
    let mut comb = Comb::open(&root).unwrap();
    let finishing = Arc::new(AtomicBool::new(false));
    if slot == 7 {
        comb.before_index = Some(Box::new(|| std::process::exit(73)));
    } else if slot >= 8 {
        comb.disk.set_durability(Arc::new(Finalization {
            exit: slot == 8,
            started: finishing.clone(),
        }));
    }
    hooks::with(
        move |at, step| {
            if at.file_name().unwrap() == "intent.json" && step == Step::AfterPublish {
                // Keep the exact preparation independently of later intent removal.
                atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
                    .write(root.join(".pair-prepared.json"), fs::read(at).unwrap())
                    .unwrap();
            }
            if slot < 7 && at == path && step == STEPS[position] {
                std::process::exit(73);
            }
            if slot == 9
                && at == path
                && finishing.load(Ordering::SeqCst)
                && step == FINISH[position]
            {
                std::process::exit(73);
            }
            Ok(())
        },
        || comb.consolidate(&request, &write("pair")).unwrap(),
    );
    panic!("consolidation death boundary was not reached");
}

struct Finalization {
    exit: bool,
    started: Arc<AtomicBool>,
}
impl Durability for Finalization {
    fn fsync(&self, file: &File) -> io::Result<()> {
        System.fsync(file)
    }
    fn ordering_barrier(&self, file: &File) -> io::Result<()> {
        System.ordering_barrier(file)
    }
    fn full_flush(&self, file: &File) -> io::Result<()> {
        System.full_flush(file)
    }
    fn sync_directory(&self, file: &File) -> io::Result<()> {
        System.sync_directory(file)
    }
    fn before_finish(&self) -> Result<()> {
        self.started.store(true, Ordering::SeqCst);
        if self.exit {
            std::process::exit(73);
        }
        Ok(())
    }
}

#[test]
fn process_death_at_each_pair_publication_and_sql_boundary_recovers_exact_preparation() {
    for slot in 0..10 {
        let positions = if slot < 7 {
            STEPS.len()
        } else if slot == 9 {
            FINISH.len()
        } else {
            1
        };
        for position in 0..positions {
            let fixture = Fixture::new();
            let path = fixture.paths()[if slot < 7 { slot } else { 0 }].clone();
            // Release the parent's lease while retaining the fixture root/request.
            let Fixture {
                root,
                request,
                comb,
            } = fixture;
            drop(comb);
            crash(root.path(), &request, &path, slot, position);
            let intent = root.path().join(".pair-prepared.json");
            let pair = if intent.exists() {
                let super::super::super::Record::Consolidation(pair) =
                    serde_json::from_slice(&fs::read(intent).unwrap()).unwrap()
                else {
                    panic!()
                };
                Some(pair)
            } else {
                None
            };
            assert_eq!(pair.is_some(), slot != 0 || position >= 4);
            if slot == 9 {
                assert_eq!(root.path().join(".comb/intent.json").exists(), position < 3);
            }
            let comb = Comb::open(root.path()).unwrap();
            Fixture {
                root,
                comb,
                request,
            }
            .recover(pair);
        }
    }
}
