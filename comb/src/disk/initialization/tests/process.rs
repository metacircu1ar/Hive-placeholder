use super::*;
use crate::disk::System;
use std::{
    process::{Child, Command, Stdio},
    time::Instant,
};

const STEPS: [Step; 7] = [
    Step::Written,
    Step::BeforeFileSync,
    Step::AfterFileSync,
    Step::BeforePublish,
    Step::AfterPublish,
    Step::BeforeDirectorySync,
    Step::AfterDirectorySync,
];
mod pages;
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
            self.0.wait().unwrap();
        }
    }
}
fn crash(root: &Path, mode: &str, index: usize) {
    let mut child = Process(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "disk::initialization::tests::process::crash_fixture",
                "--ignored",
                "--nocapture",
            ])
            .env("COMB_PUBLICATION_ROOT", root)
            .env("COMB_PUBLICATION_MODE", mode)
            .env("COMB_PUBLICATION_STEP", index.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert_eq!(status.code(), Some(73), "{mode}/{index}");
            break;
        }
        assert!(Instant::now() < deadline, "owned crash fixture timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn process_death_keeps_protected_secret_preparations_and_complete_published_identity() {
    for index in 0..BOUNDARIES.len() {
        let root = tempfile::tempdir().unwrap();
        crash(root.path(), "initialize", index);
        let before = if index >= PUBLISHED {
            Some(complete(root.path()))
        } else {
            assert!(!root.path().join(".comb").exists());
            None
        };
        let stages = preparation(root.path());
        assert_eq!(stages.len(), usize::from(index < PUBLISHED));
        for path in &stages {
            protection(path);
        }
        if index == 7 {
            assert!(
                fs::read_dir(&stages[0]).unwrap().any(|entry| {
                    let path = entry.unwrap().path();
                    path.is_file()
                        && path
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with(".comb-tmp-")
                        && fs::read(path).unwrap().windows(5).any(|s| s == b"\"key\"")
                }),
                "identity temporary survives process death"
            );
        }
        drop(Disk::acquire(root.path(), true).unwrap());
        if let Some(before) = before {
            assert_eq!(complete(root.path()), before);
        }
        for path in stages {
            protection(&path);
        }
    }
}

#[test]
fn replacement_process_death_keeps_complete_versions_and_current_preparation_names() {
    for index in 0..STEPS.len() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("state");
        atomic(&target, b"before", &System).unwrap();
        crash(root.path(), "replace", index);
        assert_eq!(
            fs::read(&target).unwrap(),
            if index >= 4 {
                b"after".as_slice()
            } else {
                b"before"
            }
        );
        let stages = preparation(root.path());
        assert_eq!(stages.len(), usize::from(index < 4));
        for path in &stages {
            assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o600);
        }
        atomic(&target, b"retry", &System).unwrap();
        assert_eq!(fs::read(target).unwrap(), b"retry");
        assert!(stages.iter().all(|p| p.exists()));
    }
}

#[test]
#[ignore = "owned process-exit fixture exercised by Comb initialization/replacement/page crash tests"]
fn crash_fixture() {
    let root = PathBuf::from(std::env::var_os("COMB_PUBLICATION_ROOT").unwrap());
    let mode = std::env::var("COMB_PUBLICATION_MODE").unwrap();
    let index: usize = std::env::var("COMB_PUBLICATION_STEP")
        .unwrap()
        .parse()
        .unwrap();
    let initialize = mode == "initialize";
    if mode == "page-create" {
        pages::exit_during_create(&root, index);
    }
    if mode == "directory" {
        let fault = Arc::new(super::directories::DirectoryFault {
            root: root.clone(),
            leaf: super::directories::LEAVES[index / 2],
            after: index % 2 == 1,
            crash: true,
        });
        drop(Disk::acquire_with(&root, true, fault).unwrap());
        panic!("directory sync death boundary not reached");
    }
    hooks::with(
        move |path, step| {
            if (initialize && selected(path, step, index)) || (!initialize && step == STEPS[index])
            {
                std::process::exit(73);
            }
            Ok(())
        },
        || {
            if initialize {
                drop(Disk::acquire(&root, true).unwrap());
            } else {
                atomic(&root.join("state"), b"after", &System).unwrap();
            }
        },
    );
    panic!("publication death boundary not reached");
}

#[test]
fn death_before_and_after_each_prepublication_directory_sync_keeps_protection() {
    for index in 0..6 {
        let root = tempfile::tempdir().unwrap();
        crash(root.path(), "directory", index);
        assert!(!root.path().join(".comb").exists());
        let stages = preparation(root.path());
        assert_eq!(stages.len(), 1);
        protection(&stages[0]);
        let before = fs::read(stages[0].join("identity.json")).unwrap();
        drop(Disk::acquire(root.path(), true).unwrap());
        complete(root.path());
        assert_eq!(fs::read(stages[0].join("identity.json")).unwrap(), before);
    }
}
