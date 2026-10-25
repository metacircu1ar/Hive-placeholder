use super::*;
use crate::test_support::{ChildGuard, rerun_isolated, wait_until};
use std::{process::Stdio, time::Duration};

#[test]
fn process_death_at_each_publication_barrier_never_leaves_an_empty_final_database() {
    for (index, at) in STEPS.into_iter().enumerate() {
        let temporary = crate::runtime::test_directory();
        let path = temporary.path().join("runtime.sqlite");
        let mut command = rerun_isolated(
            "runtime::store::files::creation::tests::process::crash_fixture",
            true,
        );
        command
            .env("HIVE_DATABASE_CRASH_ROOT", temporary.path())
            .env("HIVE_DATABASE_CRASH_BOUNDARY", index.to_string())
            .stdout(Stdio::null());
        let mut child = ChildGuard(crate::os::spawn(&mut command).unwrap());
        let status = wait_until(Duration::from_secs(10), Duration::from_millis(5), || {
            child.try_wait().unwrap()
        });
        assert_eq!(status.code(), Some(73), "did not reach {at:?}");
        assert_eq!(path.exists(), index >= 4);
        no_sidecars(&path);
        if path.exists() {
            complete(&path);
        }
        let stages: Vec<_> = fs::read_dir(temporary.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|entry| entry != &path)
            .collect();
        assert_eq!(stages.len(), usize::from(index < 4));
        for stage in &stages {
            assert!(
                stage
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with(".hive-tmp-")
            );
            assert_eq!(fs::metadata(stage).unwrap().mode() & 0o777, 0o600);
        }
        retry(&path);
        for stage in stages {
            assert!(stage.exists());
        }
    }
}

#[test]
#[ignore = "owned subprocess fixture; exercised at every boundary by process_death_at_each_publication_barrier"]
fn crash_fixture() {
    let root = PathBuf::from(std::env::var_os("HIVE_DATABASE_CRASH_ROOT").unwrap());
    let _owner = std::env::var_os("HIVE_DATABASE_OWNED")
        .map(|_| crate::state::Lock::acquire(&root.join("daemon.lock")).unwrap());
    let index: usize = std::env::var("HIVE_DATABASE_CRASH_BOUNDARY")
        .unwrap()
        .parse()
        .unwrap();
    with_hook(
        move |step| {
            if step == STEPS[index] {
                std::process::exit(73);
            }
            Ok(())
        },
        || Store::open(&root.join("runtime.sqlite")),
    )
    .unwrap();
    panic!("crash barrier was not reached");
}

#[test]
fn concurrent_process_creators_have_one_no_replace_winner() {
    let temporary = crate::runtime::test_directory();
    let root = temporary.path();
    let mut children: Vec<_> = (0..2)
        .map(|index| {
            let mut command = rerun_isolated(
                "runtime::store::files::creation::tests::process::race_fixture",
                true,
            );
            command
                .env("HIVE_DATABASE_RACE_ROOT", root)
                .env("HIVE_DATABASE_RACE_INDEX", index.to_string())
                .stdout(Stdio::null());
            ChildGuard(crate::os::spawn(&mut command).unwrap())
        })
        .collect();
    wait_until(Duration::from_secs(10), Duration::from_millis(5), || {
        (root.join("ready-0").exists() && root.join("ready-1").exists()).then_some(())
    });
    assert!(!root.join("runtime.sqlite").exists());
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .create_new(root.join("go"), b"")
        .unwrap();
    let mut codes: Vec<_> = children
        .iter_mut()
        .map(|child| {
            wait_until(Duration::from_secs(10), Duration::from_millis(5), || {
                child.try_wait().unwrap()
            })
            .code()
            .unwrap()
        })
        .collect();
    codes.sort_unstable();
    assert_eq!(codes, [0, 74]);
    retry(&root.join("runtime.sqlite"));
}

#[test]
#[ignore = "owned subprocess fixture; exercised by concurrent_process_creators"]
fn race_fixture() {
    let root = PathBuf::from(std::env::var_os("HIVE_DATABASE_RACE_ROOT").unwrap());
    let index = std::env::var("HIVE_DATABASE_RACE_INDEX").unwrap();
    let path = root.join("runtime.sqlite");
    let result = with_hook(
        move |step| {
            if step == Step::BeforePublish {
                atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
                    .create_new(root.join(format!("ready-{index}")), b"")
                    .unwrap();
                wait_until(Duration::from_secs(10), Duration::from_millis(5), || {
                    root.join("go").exists().then_some(())
                });
            }
            Ok(())
        },
        || Store::open(&path),
    );
    match result {
        Ok(store) => drop(store),
        Err(error) => {
            let failure = publication(&error);
            assert!(!failure.published());
            assert_eq!(failure.io_error().kind(), io::ErrorKind::AlreadyExists);
            std::process::exit(74);
        }
    }
}

#[test]
fn owned_database_preparation_left_by_death_is_removed_on_startup_retry() {
    let temporary = crate::runtime::test_directory();
    let state = crate::state::State::new(temporary.path().join("state")).unwrap();
    drop(state.initialize().unwrap());
    let runtime = state.root.join("runtime");
    crate::state::directory(&runtime).unwrap();
    let mut command = rerun_isolated(
        "runtime::store::files::creation::tests::process::crash_fixture",
        true,
    );
    command
        .env("HIVE_DATABASE_CRASH_ROOT", &runtime)
        .env("HIVE_DATABASE_CRASH_BOUNDARY", "0")
        .env("HIVE_DATABASE_OWNED", "1")
        .stdout(Stdio::null());
    let mut child = ChildGuard(crate::os::spawn(&mut command).unwrap());
    let status = wait_until(Duration::from_secs(10), Duration::from_millis(5), || {
        child.try_wait().unwrap()
    });
    assert_eq!(status.code(), Some(73));
    let stages: Vec<_> = fs::read_dir(&runtime)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .is_some_and(hive_setup_support::names::state_preparation_name)
        })
        .collect();
    assert_eq!(stages.len(), 1);
    let owner = crate::state::Lock::acquire(&runtime.join("daemon.lock")).unwrap();
    state.cleanup_preparations(&owner).unwrap();
    assert!(!stages[0].exists());
    retry(&runtime.join("runtime.sqlite"));
}
