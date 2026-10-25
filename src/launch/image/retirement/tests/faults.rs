use super::*;
use crate::test_support::{ChildGuard, rerun_isolated, wait_until};
use std::{
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Copy, Debug)]
struct Case(&'static str, Step, usize);
fn cases() -> Vec<Case> {
    let mut cases = vec![
        Case("original", Step::BeforePublish, 1),
        Case("original", Step::AfterPublish, 1),
        Case("original", Step::BeforeRemove, 1),
        Case("original", Step::AfterRemove, 1),
    ];
    for step in [Step::BeforeDirectorySync, Step::AfterDirectorySync] {
        cases.extend([
            Case("original", step, 1),
            Case("original", step, 2),
            Case("aside", step, 1),
        ]);
    }
    for name in ["hive-launch", ".hive-tmp-0123456789abcdef", "owner.lock"] {
        for step in [Step::BeforeRemove, Step::AfterRemove] {
            cases.push(Case(name, step, 1));
        }
    }
    cases
}
fn hook(
    image: &Image,
    case: Case,
    die: bool,
    hits: Arc<AtomicUsize>,
) -> impl Fn(&Path, Step) -> io::Result<()> + 'static {
    let path = match case.0 {
        "original" => image.directory.clone(),
        "aside" => image.retirement.aside.clone(),
        name => image.retirement.aside.join(name),
    };
    let seen = Cell::new(0);
    move |at, step| {
        if at == path && step == case.1 {
            seen.set(seen.get() + 1);
            if seen.get() == case.2 {
                hits.fetch_add(1, Ordering::SeqCst);
                if die {
                    std::process::exit(73);
                }
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
        }
        Ok(())
    }
}

#[test]
fn every_retirement_fault_retains_retry_authority_and_syncs_before_losing_owner() {
    let _fork = crate::os::fork_guard().unwrap();
    for case in cases() {
        let root = private_root().unwrap();
        let image = fixture(root.path());
        let hits = Arc::new(AtomicUsize::new(0));
        let fail = hook(&image, case, false, hits.clone());
        assert!(
            state::sync_tests::with_hook(fail, || image.cleanup()).is_err(),
            "{case:?}"
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1, "{case:?}");
        if case.0 == "aside" {
            assert!(image.retirement.aside.join("owner.lock").exists());
            assert!(!image.retirement.aside.join("hive-launch").exists());
        }
        if case.0 == "owner.lock" && case.1 == Step::AfterRemove {
            assert!(image.retirement.aside.is_dir());
            assert_eq!(fs::read_dir(&image.retirement.aside).unwrap().count(), 0);
        }
        let retirement = abandon(image);
        sweep(root.path());
        assert!(
            !retirement.original.exists() && !retirement.aside.exists(),
            "{case:?}"
        );
    }
}

#[test]
fn owned_process_death_at_each_boundary_is_reclaimed_by_next_sweep() {
    for index in 0..cases().len() {
        let root = private_root().unwrap();
        let mut command = rerun_isolated(
            "launch::image::retirement::tests::faults::crash_fixture",
            true,
        );
        command
            .env("HIVE_RETIRE_ROOT", root.path())
            .env("HIVE_RETIRE_CASE", index.to_string())
            .stdout(Stdio::null());
        let mut child = ChildGuard(crate::os::spawn(&mut command).unwrap());
        let status = wait_until(Duration::from_secs(10), Duration::from_millis(5), || {
            child.try_wait().unwrap()
        });
        assert_eq!(status.code(), Some(73), "case {index}");
        sweep(root.path());
        assert!(
            !fs::read_dir(root.path())
                .unwrap()
                .any(|entry| entry.unwrap().path().is_dir()),
            "case {index}"
        );
    }
}

#[test]
#[ignore = "owned process-death fixture, invoked by retirement recovery test"]
fn crash_fixture() {
    let root = PathBuf::from(std::env::var_os("HIVE_RETIRE_ROOT").unwrap());
    let index: usize = std::env::var("HIVE_RETIRE_CASE").unwrap().parse().unwrap();
    let image = fixture(&root);
    let fail = hook(&image, cases()[index], true, Arc::new(AtomicUsize::new(0)));
    state::sync_tests::with_hook(fail, || image.cleanup()).unwrap();
    panic!("death boundary was not reached");
}

#[test]
fn owner_creation_is_durable_before_copy_and_failures_never_reach_payload_creation() {
    let _fork = crate::os::fork_guard().unwrap();
    for step in [
        Step::BeforeFileSync,
        Step::AfterFileSync,
        Step::BeforeDirectorySync,
        Step::AfterDirectorySync,
    ] {
        let root = private_root().unwrap();
        fs::write(root.path().join("source"), b"launcher").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let reached = calls.clone();
        let result = state::sync_tests::with_hook(
            move |path, at| {
                if path.file_name().is_some_and(|n| n == "owner.lock") && at == step {
                    reached.fetch_add(1, Ordering::SeqCst);
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
                Ok(())
            },
            || {
                build_image(root.path(), &root.path().join("source"), |_| {
                    panic!("copy before owner durability")
                })
            },
        );
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        sweep(root.path());
        assert!(
            !fs::read_dir(root.path())
                .unwrap()
                .any(|entry| entry.unwrap().path().is_dir())
        );
    }
}
