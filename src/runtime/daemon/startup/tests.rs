use super::*;
use crate::runtime::Binaries;

#[test]
fn invalid_logging_limits_are_rejected_before_state_creation() {
    let temp = crate::runtime::test_directory();
    let config = temp.path().join("invalid.yaml");
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .create_new(&config, b"auth_profiles: {}\nlogging: {files: 1}\n")
        .unwrap();
    let root = temp.path().join("state");
    let error = OwnedDaemon::open(config, root.clone(), Binaries::default())
        .err()
        .unwrap();
    assert_eq!(error.code(), "config_invalid");
    assert!(!root.exists());
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    let missing_parent = temp.path().join("missing");
    assert!(
        OwnedDaemon::open(
            temp.path().join("invalid.yaml"),
            missing_parent.join("state"),
            Binaries::default()
        )
        .is_err()
    );
    assert!(!missing_parent.exists());
}

#[test]
fn startup_sweep_requires_idle_owner_and_original_lock_evidence() {
    for kind in ["held", "live", "replaced", "idle"] {
        let mut fork = Some(crate::os::fork_guard().unwrap());
        let temp = crate::runtime::test_directory();
        let root = temp.path().join("state");
        let runtime = root.join("runtime");
        drop(State::new(root.clone()).unwrap().initialize().unwrap());
        state::directory(&runtime).unwrap();
        let config = temp.path().join("config.yaml");
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .create_new(&config, b"auth_profiles: {}\n")
            .unwrap();
        let lock_path = runtime.join("daemon.lock");
        let lock = Lock::acquire(&lock_path).unwrap();
        let candidate = runtime.join(".hive-tmp-state-ABC123");
        state::private_file(&candidate, true).unwrap();
        if kind == "live" {
            write_daemon_owner(
                &runtime.join("owner.json"),
                &state::boot_identity().unwrap(),
                &lock,
                &"0".repeat(64),
            )
            .unwrap();
        }
        let held = (kind == "held").then_some(lock);
        if kind == "replaced" {
            let mut command = crate::test_support::rerun_isolated(
                "runtime::daemon::owner::owner_tests::write_owner_marker_fixture",
                true,
            );
            command.env("HIVE_OWNER_MARKER_TEST_ROOT", &runtime);
            drop(fork.take());
            crate::test_support::run_isolated(&mut command);
            fork = Some(crate::os::fork_guard().unwrap());
            fs::rename(&lock_path, runtime.join("old.lock")).unwrap();
            state::private_file(&lock_path, true).unwrap();
        }
        let opened = OwnedDaemon::open(
            config,
            root,
            Binaries {
                codex: "/missing-fixture-codex".into(),
                claude: "/missing-fixture-claude".into(),
            },
        );
        if kind == "idle" {
            opened.unwrap();
            assert!(!candidate.exists());
        } else {
            let code = opened.err().unwrap().code();
            assert_eq!(
                code,
                if kind == "held" {
                    "daemon_busy"
                } else {
                    "recovery_required"
                }
            );
            assert!(candidate.exists());
        }
        drop(held);
        drop(fork);
    }
}
