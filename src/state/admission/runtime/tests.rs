use super::*;
use crate::state::{Binding, directory, private_file};
use hive_setup_support::admission::Exclusive;

fn fixture() -> (tempfile::TempDir, State, Binding) {
    let (temp, state, binding) = crate::state::admission::tests::fixture();
    let runtime = state.root.join("runtime");
    directory(&runtime).unwrap();
    let path = runtime.join("runtime.sqlite");
    drop(private_file(&path, true).unwrap());
    let connection = rusqlite::Connection::open(&path).unwrap();
    crate::runtime::initialize_schema(&connection).unwrap();
    drop(connection);
    let admission = state.admission().unwrap();
    let daemon = Arc::new(
        Lock::acquire(&runtime.join("daemon.lock"))
            .unwrap()
            .with_admission(admission),
    );
    let state = state.for_runtime(daemon).unwrap();
    (temp, state, binding)
}

#[test]
fn runtime_reuses_database_validation_but_reads_current_profile_records() {
    let (_temp, state, binding) = fixture();
    let reads = crate::runtime::read_connection_count();
    for _ in 0..5 {
        let checks = CHECKS.get();
        let metadata = state.metadata_lock().unwrap();
        assert_eq!(CHECKS.get() - checks, 1);
        let selected = state.selected_generation("main", &binding.profile).unwrap();
        drop(metadata);
        let lease = state.runtime_lease(&selected).unwrap();
        lease.verify(&state, &selected).unwrap();
        // One admission for each metadata transaction and one for the lease,
        // including both fresh credential observations; never a duplicate check.
        assert_eq!(CHECKS.get() - checks, 6);
    }
    assert_eq!(crate::runtime::read_connection_count(), reads);
    let cold = State::new(state.root.clone()).unwrap();
    cold.check_format().unwrap();
    assert!(crate::runtime::read_connection_count() > reads);
    std::fs::write(state.root.join("index.json"), b"{}").unwrap();
    let metadata = state.metadata_lock().unwrap();
    assert!(state.selected_generation("main", &binding.profile).is_err());
    drop(metadata);
}

#[test]
fn detached_state_keeps_daemon_and_migration_exclusion_until_its_last_clone() {
    // Concurrent test spawns must not inherit the descriptor being released.
    let _fork = crate::os::fork_guard().unwrap();
    let (_temp, state, _) = fixture();
    let root = state.root.clone();
    let clone = state.clone();
    drop(state);
    assert!(Lock::acquire(&root.join("runtime/daemon.lock")).is_err());
    assert!(Exclusive::existing(&root, "hive-state").is_err());
    clone.check_format().unwrap();
    drop(clone);
    drop(Lock::acquire(&root.join("runtime/daemon.lock")).unwrap());
    drop(Exclusive::existing(&root, "hive-state").unwrap());
}

#[test]
fn runtime_rejects_replaced_ownership_database_marker_and_pending_migration() {
    for target in [
        "runtime/runtime.sqlite",
        "runtime/daemon.lock",
        "format",
        "migration-intent.json",
    ] {
        let (_temp, state, _) = fixture();
        let path = state.root.join(target);
        if target.starts_with("runtime/") {
            std::fs::rename(&path, path.with_extension("old")).unwrap();
            drop(private_file(&path, true).unwrap());
        } else {
            std::fs::write(&path, b"{}").unwrap();
        }
        assert!(state.check_format().is_err(), "{target}");
        assert!(state.metadata_lock().is_err(), "{target}");
    }
    let (temp, state, _) = fixture();
    std::fs::rename(&state.root, temp.path().join("old-root")).unwrap();
    directory(&state.root).unwrap();
    assert!(state.check_format().is_err());
}
