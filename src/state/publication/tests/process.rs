use super::*;
use crate::test_support::{ChildGuard, rerun_isolated, wait_until};
use std::{process::Stdio, time::Duration};

#[test]
fn process_exit_at_each_boundary_retains_complete_state_and_recognizable_preparations() {
    for index in 0..STEPS.len() {
        let (_temp, path) = fixture();
        let mut command = rerun_isolated("state::publication::tests::process::crash_fixture", true);
        command
            .env("HIVE_JSON_CRASH_PATH", &path)
            .env("HIVE_JSON_CRASH_BOUNDARY", index.to_string())
            .stdout(Stdio::null());
        let mut child = ChildGuard(crate::os::spawn(&mut command).unwrap());
        let status = wait_until(Duration::from_secs(10), Duration::from_millis(5), || {
            child.try_wait().unwrap()
        });
        assert_eq!(status.code(), Some(73), "boundary {index}");
        assert_eq!(fs::read(&path).unwrap(), if index >= 4 { NEW } else { OLD });
        let stages: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|entry| entry != &path)
            .collect();
        assert_eq!(stages.len(), usize::from(index < 4));
        for stage in &stages {
            assert!(staging_file(stage).unwrap());
            assert_eq!(fs::metadata(stage).unwrap().mode() & 0o777, 0o600);
        }
        write_json(&path, &json!({"new":true})).unwrap();
        assert_eq!(fs::read(&path).unwrap(), NEW);
        // Recognition is not a sweep: interrupted preparations remain untouched.
        assert!(stages.iter().all(|stage| stage.exists()));
    }
}

#[test]
#[ignore = "owned process-exit fixture; exercised by the boundary recovery test"]
fn crash_fixture() {
    let path = PathBuf::from(std::env::var_os("HIVE_JSON_CRASH_PATH").unwrap());
    let index: usize = std::env::var("HIVE_JSON_CRASH_BOUNDARY")
        .unwrap()
        .parse()
        .unwrap();
    let expected = path.clone();
    state::sync_tests::with_hook(
        move |path, step| {
            if path == expected && step == STEPS[index] {
                std::process::exit(73);
            }
            Ok(())
        },
        || write_json(&path, &json!({"new":true})),
    )
    .unwrap();
    panic!("exit boundary was not reached");
}
