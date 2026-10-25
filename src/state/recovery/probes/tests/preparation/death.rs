use super::*;
use crate::test_support::{ChildGuard, rerun_isolated, wait_until};
use std::{process::Stdio, time::Duration};

#[test]
fn owner_absence_reclaims_preparation_after_death_at_each_durable_boundary() {
    for index in 0..8 {
        let temporary = tempfile::tempdir().unwrap();
        let state = State::new(temporary.path().join("state")).unwrap();
        let mut command = rerun_isolated(
            "state::recovery::probes::tests::preparation::death::crash_fixture",
            true,
        );
        command
            .env("HIVE_PROBE_PREPARATION_ROOT", &state.root)
            .env("HIVE_PROBE_PREPARATION_BOUNDARY", index.to_string())
            .stdout(Stdio::null());
        let mut child = ChildGuard(crate::os::spawn(&mut command).unwrap());
        let status = wait_until(Duration::from_secs(10), Duration::from_millis(5), || {
            child.try_wait().unwrap()
        });
        assert_eq!(status.code(), Some(73));
        let entries: Vec<_> = fs::read_dir(state.root.join("native-probes"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(entries.len(), 1);
        let dir = &entries[0];
        let manifest: Manifest = read_json(&dir.join("identity.json")).unwrap();
        assert!(manifest.owner.absent().unwrap());
        assert_eq!(matches!(manifest.launch, Launch::Receipt(_)), index >= 6);
        let records = active(dir);
        assert_eq!(records.len(), usize::from(index >= 2));
        assert!(
            records
                .iter()
                .all(|r| matching(&manifest, r) && pristine(r))
        );
        state.sweep_metadata_probes();
        assert!(!dir.exists(), "boundary {index}");
    }
}

#[test]
#[ignore = "owned subprocess fixture; exercised by owner_absence_reclaims_preparation"]
fn crash_fixture() {
    let state = State::new(
        std::env::var_os("HIVE_PROBE_PREPARATION_ROOT")
            .unwrap()
            .into(),
    )
    .unwrap();
    let index: usize = std::env::var("HIVE_PROBE_PREPARATION_BOUNDARY")
        .unwrap()
        .parse()
        .unwrap();
    let mut operation = state.metadata_probe().unwrap();
    with_hook(
        move |path, at| {
            if selected(path, index >= 4) && at == STEPS[index % 4] {
                std::process::exit(73);
            }
            Ok(())
        },
        || operation.prepare_child(true, false),
    )
    .unwrap();
    panic!("crash boundary not reached");
}
