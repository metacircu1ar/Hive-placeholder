use super::*;
use crate::state::sync_tests::with_hook;
use atomic_fs::Step;
use std::{cell::Cell, io, path::PathBuf};
mod death;
mod refusal;

#[derive(Clone, Copy, Debug)]
struct Fault {
    permit: bool,
    step: Step,
    occurrence: usize,
}
fn faults() -> Vec<Fault> {
    let mut faults = Vec::new();
    for step in [
        Step::Written,
        Step::BeforeFileSync,
        Step::AfterFileSync,
        Step::BeforePublish,
        Step::AfterPublish,
        Step::BeforeDirectorySync,
        Step::AfterDirectorySync,
    ] {
        faults.push(Fault {
            permit: true,
            step,
            occurrence: 1,
        });
    }
    for step in [
        Step::BeforeFileSync,
        Step::AfterFileSync,
        Step::BeforeDirectorySync,
        Step::AfterDirectorySync,
    ] {
        faults.push(Fault {
            permit: true,
            step,
            occurrence: 2,
        });
    }
    for (step, occurrence) in [
        (Step::BeforePublish, 1),
        (Step::AfterPublish, 1),
        (Step::BeforeDirectorySync, 1),
        (Step::AfterDirectorySync, 1),
        (Step::BeforeRemove, 1),
        (Step::AfterRemove, 1),
        (Step::BeforeDirectorySync, 2),
        (Step::AfterDirectorySync, 2),
    ] {
        faults.push(Fault {
            permit: false,
            step,
            occurrence,
        });
    }
    for (step, occurrence) in [
        (Step::BeforeRemove, 1),
        (Step::AfterRemove, 1),
        (Step::BeforeDirectorySync, 3),
        (Step::AfterDirectorySync, 3),
    ] {
        faults.push(Fault {
            permit: true,
            step,
            occurrence,
        });
    }
    faults
}
fn selected(fault: Fault, seen: &Cell<usize>, path: &Path, step: Step) -> bool {
    let name = path.file_name().unwrap().to_str().unwrap();
    let matches = if fault.permit {
        name.ends_with(".prune.json")
    } else {
        Uuid::parse_str(name).is_ok()
    };
    if matches && step == fault.step {
        seen.set(seen.get() + 1);
        seen.get() == fault.occurrence
    } else {
        false
    }
}
fn fixture() -> (tempfile::TempDir, State, PathBuf, Uuid) {
    let (tmp, state, operation, dir) = preparation::fixture();
    let id = dir.file_name().unwrap().to_str().unwrap().parse().unwrap();
    state::private_file(&dir.join("provider/fact"), true).unwrap();
    drop(operation);
    (tmp, state, dir, id)
}
fn removed(dir: &Path) {
    let id = dir.file_name().unwrap().to_str().unwrap();
    assert!(!dir.exists());
    assert!(!dir.with_file_name(format!("{id}.pruning")).exists());
    assert!(!dir.with_file_name(format!("{id}.prune.json")).exists());
}
fn fail(state: &State, dir: &Path, id: Uuid, fault: Fault) {
    let seen = Cell::new(0);
    let error = with_hook(
        move |path, step| {
            if selected(fault, &seen, path, step) {
                Err(io::Error::from_raw_os_error(libc::EIO))
            } else {
                Ok(())
            }
        },
        || finish_probe(state, dir, id),
    )
    .unwrap_err();
    let mut cause: &dyn std::error::Error = &error;
    while cause.downcast_ref::<io::Error>().is_none() {
        cause = cause.source().unwrap();
    }
    assert_eq!(
        cause.downcast_ref::<io::Error>().unwrap().raw_os_error(),
        Some(libc::EIO)
    );
}
#[test]
fn every_publication_removal_and_retry_barrier_preserves_recoverability() {
    let _fork = crate::os::fork_guard().unwrap();
    for fault in faults() {
        let (_tmp, state, dir, id) = fixture();
        fail(&state, &dir, id, fault);
        finish_probe(&state, &dir, id).unwrap_or_else(|e| panic!("{fault:?}: {e:?}"));
        removed(&dir);
        finish_probe(&state, &dir, id).unwrap();
        removed(&dir);
    }
}
#[test]
fn partial_removal_survives_missing_ownership_evidence_and_absence_sync_failure() {
    let _fork = crate::os::fork_guard().unwrap();
    let (_tmp, state, dir, id) = fixture();
    let aside = dir.with_file_name(format!("{id}.pruning"));
    let partial = aside.clone();
    let seen = Cell::new(0);
    let fault = Fault {
        permit: false,
        step: Step::BeforeRemove,
        occurrence: 1,
    };
    with_hook(
        move |path, step| {
            if selected(fault, &seen, path, step) {
                fs::remove_file(partial.join("auth.lock"))?;
                fs::remove_file(partial.join("identity.json"))?;
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
            Ok(())
        },
        || finish_probe(&state, &dir, id),
    )
    .unwrap_err();
    assert!(aside.join("provider/fact").exists());
    let seen = Cell::new(0);
    let fault = Fault {
        permit: false,
        step: Step::BeforeDirectorySync,
        occurrence: 2,
    };
    with_hook(
        move |path, step| {
            if selected(fault, &seen, path, step) {
                Err(io::Error::from_raw_os_error(libc::EIO))
            } else {
                Ok(())
            }
        },
        || finish_probe(&state, &dir, id),
    )
    .unwrap_err();
    assert!(!aside.exists());
    assert!(dir.with_file_name(format!("{id}.prune.json")).exists());
    state.sweep_metadata_probes_checked().unwrap();
    removed(&dir);
}
#[test]
fn concurrent_finish_and_sweep_serialize_on_the_publication_lock() {
    let _fork = crate::os::fork_guard().unwrap();
    let (_tmp, state, dir, id) = fixture();
    let (arrived, waiting) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let first = State::new(state.root.clone()).unwrap();
    let copied = dir.clone();
    let worker = std::thread::spawn(move || {
        with_hook(
            move |path, step| {
                if path.extension().is_some_and(|e| e == "json") && step == Step::Written {
                    arrived.send(()).unwrap();
                    released.recv().unwrap();
                }
                Ok(())
            },
            || finish_probe(&first, &copied, id),
        )
    });
    waiting
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let candidate = super::temps::stages(&state).pop().unwrap();
    assert!(!dir.with_file_name(format!("{id}.prune.json")).exists());
    let (admitted, admission) = std::sync::mpsc::channel();
    let (done, completed) = std::sync::mpsc::channel();
    let sweeper = std::thread::spawn(move || {
        let result = with_hook(
            move |path, step| {
                if path.file_name().unwrap() == "native-probes.lock" && step == Step::BeforePublish
                {
                    admitted.send(()).unwrap();
                }
                Ok(())
            },
            || state.sweep_metadata_probes_checked(),
        );
        done.send(()).unwrap();
        result
    });
    admission
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert!(matches!(
        completed.recv_timeout(std::time::Duration::from_millis(100)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    assert!(
        candidate.exists(),
        "the active permit preparation must survive"
    );
    release.send(()).unwrap();
    completed
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    worker.join().unwrap().unwrap();
    sweeper.join().unwrap().unwrap();
    removed(&dir);
}
