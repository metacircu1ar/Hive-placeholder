use super::*;
use crate::{
    Actor, Comb,
    disk::publication_hooks as hooks,
    values::{ContentHash, Slug},
};
use atomic_fs::Step;
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const BEFORE: &str = "---\ntitle: Page\n---\nBefore";
const AFTER: &str = "---\ntitle: Page\n---\nAfter";
const STEPS: [Step; 9] = [
    Step::Written,
    Step::BeforeFileSync,
    Step::AfterFileSync,
    Step::BeforePublish,
    Step::AfterPublish,
    Step::BeforeSourceDirectorySync,
    Step::AfterSourceDirectorySync,
    Step::BeforeDirectorySync,
    Step::AfterDirectorySync,
];
fn slug() -> Slug {
    "pages/staged".parse().unwrap()
}
fn write(id: &str) -> crate::store::Write<'_> {
    crate::test_support::write(&Actor::Human, id, None).unwrap()
}
fn fixture(seed: bool) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let mut comb = Comb::init(root.path()).unwrap();
    if seed {
        comb.put_markdown(&slug(), BEFORE, None, &write("seed"))
            .unwrap();
    }
    root
}
fn mutate(comb: &mut Comb) -> Result<crate::store::Mutation> {
    let before = comb.disk.content(&slug())?.map(ContentHash::digest);
    comb.put_markdown(&slug(), AFTER, before.as_ref(), &write("attempt"))
}
fn preparations(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root.join(".comb"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            hive_setup_support::names::comb_admin_preparation_name(path.file_name().unwrap())
        })
        .collect()
}
struct Owned(Child);
impl Drop for Owned {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
            self.0.wait().unwrap();
        }
    }
}
fn die(root: &Path, step: usize) {
    let mut child = Owned(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "disk::durable::tests::page_staging::death_fixture",
                "--ignored",
            ])
            .env("COMB_PAGE_STAGE_ROOT", root)
            .env("COMB_PAGE_STAGE_STEP", step.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert_eq!(status.code(), Some(73));
            break;
        }
        assert!(Instant::now() < deadline, "owned page child timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
#[ignore = "owned process fixture invoked by page staging crash regression"]
fn death_fixture() {
    let root = PathBuf::from(std::env::var_os("COMB_PAGE_STAGE_ROOT").unwrap());
    let step: usize = std::env::var("COMB_PAGE_STAGE_STEP")
        .unwrap()
        .parse()
        .unwrap();
    let mut comb = Comb::open(&root).unwrap();
    let selected = comb.disk.page_path(&slug(), true).unwrap();
    hooks::with(
        move |path, at| {
            if path == selected && at == STEPS[step] {
                std::process::exit(73);
            }
            Ok(())
        },
        || mutate(&mut comb),
    )
    .unwrap();
    panic!("page death boundary not reached");
}
#[test]
fn owned_death_at_create_replace_and_both_parent_barriers_recovers_and_sweeps() {
    for seeded in [false, true] {
        for step in 0..STEPS.len() {
            let root = fixture(seeded);
            let path = root.path().join("pages/staged.md");
            let expected = crate::test_support::optional_bytes(&path).map(ContentHash::digest);
            die(root.path(), step);
            assert_eq!(preparations(root.path()).len(), usize::from(step < 4));
            assert_eq!(
                fs::read_dir(path.parent().unwrap()).unwrap().count(),
                usize::from(seeded || step >= 4)
            );
            let mut reopened = Comb::open(root.path()).unwrap();
            assert!(preparations(root.path()).is_empty());
            assert!(!root.path().join(".comb/intent.json").exists());
            let result = reopened
                .put_markdown(&slug(), AFTER, expected.as_ref(), &write("attempt"))
                .unwrap();
            assert_eq!(result.replayed, step >= 4);
            assert_eq!(reopened.get(&slug()).unwrap().page.compiled, "After");
        }
    }
}
#[test]
fn repeated_recovery_barrier_failure_keeps_accepted_intent_even_for_removed_page() {
    for remove in [false, true] {
        for step in [
            Step::BeforeSourceDirectorySync,
            Step::AfterSourceDirectorySync,
            Step::BeforeDirectorySync,
            Step::AfterDirectorySync,
        ] {
            let root = fixture(true);
            let mut comb = Comb::open(root.path()).unwrap();
            let recording = Arc::new(Recording::default());
            recording.fail_finish_once();
            comb.disk.set_durability(recording);
            let error = if remove {
                let hash = comb.get(&slug()).unwrap().hash;
                comb.remove(&slug(), &hash, &write("attempt")).unwrap_err()
            } else {
                mutate(&mut comb).unwrap_err()
            };
            assert!(error.file_committed);
            let page = comb.disk.page_path(&slug(), false).unwrap();
            drop(comb);
            for _ in 0..2 {
                let selected = page.clone();
                let error = hooks::with(
                    move |path, at| {
                        if path == selected && at == step {
                            return Err(io::Error::from_raw_os_error(libc::EIO));
                        }
                        Ok(())
                    },
                    || Comb::open(root.path()),
                )
                .err()
                .unwrap();
                assert_io(&error, true, None);
                assert!(root.path().join(".comb/intent.json").exists());
            }
            let reopened = Comb::open(root.path()).unwrap();
            assert!(!root.path().join(".comb/intent.json").exists());
            assert_eq!(reopened.disk.content(&slug()).unwrap().is_none(), remove);
        }
    }
}
#[test]
fn recovery_does_not_retire_intent_after_editor_changes_page_at_barrier() {
    let root = fixture(true);
    let mut comb = Comb::open(root.path()).unwrap();
    let recording = Arc::new(Recording::default());
    recording.fail_finish_once();
    comb.disk.set_durability(recording);
    assert!(mutate(&mut comb).unwrap_err().file_committed);
    let page = comb.disk.page_path(&slug(), false).unwrap();
    let selected = page.clone();
    drop(comb);
    let error = hooks::with(
        move |path, step| {
            if path == selected && step == Step::BeforeSourceDirectorySync {
                fs::write(path, b"editor third state")?;
            }
            Ok(())
        },
        || Comb::open(root.path()),
    )
    .err()
    .unwrap();
    assert!(error.file_committed);
    assert_eq!(error.code, ErrorCode::RecoveryRequired);
    assert_eq!(fs::read(page).unwrap(), b"editor third state");
    assert!(root.path().join(".comb/intent.json").exists());
    assert!(Comb::open(root.path()).is_err());
}
#[test]
fn ordinary_write_adds_exactly_one_source_parent_fsync() {
    let root = fixture(true);
    let mut comb = Comb::open(root.path()).unwrap();
    let recording = Arc::new(Recording::default());
    recording.barrier(Ok(()));
    comb.disk.set_durability(recording.clone());
    mutate(&mut comb).unwrap();
    let counts = recording.counts();
    assert_eq!(
        counts
            .events
            .iter()
            .filter(|op| **op == Operation::Fsync)
            .count(),
        10
    );
    assert_eq!(counts.full_barriers, 2);
}

#[test]
fn pending_single_keeps_accepted_status_when_later_index_admission_fails() {
    let root = fixture(true);
    let mut comb = Comb::open(root.path()).unwrap();
    let before = comb.get(&slug()).unwrap().hash;
    let recording = Arc::new(Recording::default());
    recording.fail_finish_once();
    comb.disk.set_durability(recording);
    assert!(mutate(&mut comb).unwrap_err().file_committed);
    let index = comb.disk.index_path();
    drop(comb);
    let permissions = fs::metadata(&index).unwrap().permissions();
    fs::set_permissions(&index, fs::Permissions::from_mode(0o620)).unwrap();
    let error = Comb::open(root.path()).err().unwrap();
    assert_eq!(error.code, ErrorCode::UnsafePath);
    assert!(error.file_committed);
    assert!(root.path().join(".comb/intent.json").exists());
    fs::set_permissions(&index, permissions).unwrap();
    let mut reopened = Comb::open(root.path()).unwrap();
    assert!(!root.path().join(".comb/intent.json").exists());
    assert!(
        reopened
            .put_markdown(&slug(), AFTER, Some(&before), &write("attempt"))
            .unwrap()
            .replayed
    );
}
