use super::*;
use crate::state::sync_tests::with_hook;
use atomic_fs::Step;
use std::{io, path::PathBuf};
mod death;
mod evidence;

const STEPS: [Step; 4] = [
    Step::BeforeFileSync,
    Step::AfterFileSync,
    Step::BeforeDirectorySync,
    Step::AfterDirectorySync,
];

fn selected(path: &Path, reservation: bool) -> bool {
    if reservation {
        path.file_name().unwrap() == "identity.json"
    } else {
        path.parent().unwrap().file_name().unwrap() == "executions"
    }
}

pub(super) fn fixture() -> (
    tempfile::TempDir,
    State,
    super::super::super::Operation,
    PathBuf,
) {
    let temporary = tempfile::tempdir().unwrap();
    let state = State::new(temporary.path().join("state")).unwrap();
    let operation = state.metadata_probe().unwrap();
    let dir = fs::read_dir(state.root.join("native-probes"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    (temporary, state, operation, dir)
}

fn fail_preparation(
    operation: &mut super::super::super::Operation,
    reservation: bool,
    at: Step,
) -> Error {
    with_hook(
        move |path, step| {
            if selected(path, reservation) && step == at {
                Err(io::Error::from_raw_os_error(libc::EIO))
            } else {
                Ok(())
            }
        },
        || operation.prepare_child(true, false),
    )
    .err()
    .unwrap()
}

fn active(dir: &Path) -> Vec<ledger::Record> {
    ledger::records_in(dir, "executions").unwrap()
}

#[test]
fn sync_failures_consume_the_attempt_never_launch_twice_and_allow_local_completion() {
    if std::env::var_os("HIVE_PROBE_PREPARATION_FIXTURE").is_none() {
        crate::test_support::run_isolated(crate::test_support::rerun_isolated(
            "state::recovery::probes::tests::preparation::sync_failures_consume_the_attempt_never_launch_twice_and_allow_local_completion",
            false,
        ).env("HIVE_PROBE_PREPARATION_FIXTURE", "1"));
        return;
    }
    // Immediate local cleanup requires no unrelated process to inherit the
    // fixture's credential lock. The rejected retry remains free to try a launch.
    let launcher = crate::launch::test_cache().unwrap();
    for reservation in [false, true] {
        for (index, at) in STEPS.into_iter().enumerate() {
            let (_temporary, state, mut operation, dir) = fixture();
            let error = fail_preparation(&mut operation, reservation, at);
            assert_eq!(error.io_raw_os_error(), Some(libc::EIO));
            let manifest: Manifest = read_json(&dir.join("identity.json")).unwrap();
            assert_eq!(
                matches!(manifest.launch, Launch::Receipt(_)),
                reservation && index >= 2
            );
            let records = active(&dir);
            assert_eq!(records.len(), usize::from(reservation || index >= 2));
            for record in &records {
                assert!(matching(&manifest, record) && pristine(record));
            }
            let before = fs::read(dir.join("identity.json")).unwrap();
            let receipt_bytes = records
                .first()
                .map(|r| fs::read(dir.join(format!("executions/{}.json", r.id))).unwrap());
            // Captured::start must reject before reaching native launch. Even an
            // executable that would immediately fail must yield invalid_request.
            let retry = crate::process::Captured::start(
                &launcher,
                &mut std::process::Command::new("/usr/bin/false"),
                &mut operation,
                std::time::Duration::from_secs(2),
                crate::process::Cancellation::install().unwrap(),
            )
            .err()
            .unwrap();
            assert_eq!(retry.code(), "invalid_request");
            assert_eq!(fs::read(dir.join("identity.json")).unwrap(), before);
            assert_eq!(active(&dir).len(), records.len());
            if let Some(bytes) = receipt_bytes {
                assert_eq!(
                    fs::read(dir.join(format!("executions/{}.json", records[0].id))).unwrap(),
                    bytes
                );
            }
            // Unrelated test forks can retain our CLOEXEC lock until exec. Wait
            // for that window to close before asserting immediate local cleanup.
            let fork = crate::os::fork_guard().unwrap();
            state.sweep_metadata_probes();
            assert!(dir.exists(), "ordinary sweep must retain a live owner");
            operation.finish().unwrap();
            drop(fork);
            assert!(!dir.exists(), "local completion at {reservation}/{at:?}");
        }
    }
}

#[test]
fn pure_rejection_does_not_consume_attempt_and_surviving_descriptors_block_cleanup() {
    let _fork = crate::os::fork_guard().unwrap();
    for reserved in [false, true] {
        let (_temporary, state, mut operation, dir) = fixture();
        assert_eq!(
            operation.prepare_child(true, true).err().unwrap().code(),
            "invalid_request"
        );
        let receipt = if reserved {
            Some(operation.prepare_child(true, false).unwrap())
        } else {
            fail_preparation(&mut operation, true, Step::BeforeFileSync);
            None
        };
        let inherited = operation.fd().try_clone_to_owned().unwrap();
        operation.finish().unwrap();
        assert!(dir.exists());
        state.sweep_metadata_probes();
        assert!(dir.exists());
        drop(inherited);
        if receipt.is_some() {
            assert!(Lock::acquire_existing(&dir.join("auth.lock")).is_err());
        }
        drop(receipt);
        drop(Lock::acquire_existing(&dir.join("auth.lock")).unwrap());
        state.sweep_metadata_probes();
        assert!(
            dir.exists(),
            "released locks do not prove a live owner ended"
        );
    }
}
