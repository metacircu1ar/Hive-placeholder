use super::*;
mod preparation;
mod removal;
mod temps;
mod wrapper;
fn complete(operation: &mut super::super::Operation) {
    let launcher = crate::launch::test_cache().unwrap();
    let mut child = crate::process::Captured::start(
        &launcher,
        &mut std::process::Command::new("/usr/bin/true"),
        operation,
        std::time::Duration::from_secs(5),
        crate::process::Cancellation::install().unwrap(),
    )
    .unwrap();
    assert!(child.output().unwrap().status.success());
}
#[test]
fn metadata_cleanup_requires_exact_receipt_and_released_lock() {
    for defect in [
        "none",
        "missing",
        "duplicate",
        "busy",
        "unallocated",
        "live",
        "stray",
        "symlink",
        "unknown_field",
        "missing_launch",
        "lock_identity",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let state = State::new(tmp.path().join("state")).unwrap();
        let mut operation = state.metadata_probe().unwrap();
        complete(&mut operation);
        let _fork = crate::os::fork_guard().unwrap();
        let dir = fs::read_dir(state.root.join("native-probes"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        state.sweep_metadata_probes();
        assert!(dir.exists());
        let manifest: Manifest = read_json(&dir.join("identity.json")).unwrap();
        let Launch::Receipt(id) = manifest.launch else {
            panic!("receipt required");
        };
        let archive = dir.join(format!("execution-archive/{id}.json"));
        // Duplicate the original OFD, as an inherited descriptor would.
        let retained = (defect == "busy").then(|| operation.fd().try_clone_to_owned().unwrap());
        match defect {
            "missing" => fs::remove_file(archive).unwrap(),
            "duplicate" => {
                crate::state::directory(&dir.join("executions")).unwrap();
                fs::copy(archive, dir.join(format!("executions/{id}.json"))).unwrap();
            }
            "unallocated" | "unknown_field" | "missing_launch" | "lock_identity" => {
                let path = dir.join("identity.json");
                let mut value: serde_json::Value = read_json(&path).unwrap();
                match defect {
                    "unallocated" => value["launch"] = "unallocated".into(),
                    "unknown_field" => value["unexpected"] = 1.into(),
                    "missing_launch" => {
                        value.as_object_mut().unwrap().remove("launch");
                    }
                    _ => value["lock"]["ino"] = 0.into(),
                }
                write_json(&path, &value).unwrap();
            }
            "live" => {
                let mut record: ledger::Record = read_json(&archive).unwrap();
                record.confirmed = false;
                record.quiescent = false;
                record.never_authorized = false;
                record.phase = ledger::ExecutionPhase::Authorized;
                record.process = Some(ProcessIdentity::current().unwrap());
                record.own_group = false;
                write_json(&archive, &record).unwrap();
            }
            "stray" => {
                crate::state::private_file(&dir.join("stray"), true).unwrap();
            }
            "symlink" => {
                fs::remove_dir(dir.join("provider")).unwrap();
                std::os::unix::fs::symlink(tmp.path(), dir.join("provider")).unwrap();
            }
            _ => {}
        }
        operation.finish().unwrap();
        assert_eq!(dir.exists(), defect != "none", "{defect}");
        drop(retained);
    }
}
#[test]
#[ignore = "subprocess fixture"]
fn metadata_gc_fixture() {
    let root = std::env::var_os("HIVE_PROBE_GC_FIXTURE").expect("subprocess fixture input");
    let state = State::new(root.into()).unwrap();
    let mut operation = state.metadata_probe().unwrap();
    complete(&mut operation);
    drop(operation); // Simulates caller exit without finalization, preserving receipts.
}
#[test]
fn sweep_reclaims_completed_probe_after_creator_exit() {
    let tmp = tempfile::tempdir().unwrap();
    let state = State::new(tmp.path().join("state")).unwrap();
    crate::test_support::run_isolated(
        crate::test_support::rerun_isolated(
            "state::recovery::probes::tests::metadata_gc_fixture",
            true,
        )
        .env("HIVE_PROBE_GC_FIXTURE", &state.root)
        .stdin(std::process::Stdio::null()),
    );
    assert_eq!(
        fs::read_dir(state.root.join("native-probes"))
            .unwrap()
            .count(),
        1
    );
    state.sweep_metadata_probes();
    assert_eq!(
        fs::read_dir(state.root.join("native-probes"))
            .unwrap()
            .count(),
        0
    );
}
