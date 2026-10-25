use super::*;

#[test]
fn canary_workspace_and_marker_are_private_under_permissive_umask() {
    if std::env::var_os("HIVE_CANARY_MODE_FIXTURE").is_none() {
        crate::test_support::run_isolated(crate::test_support::rerun_isolated(
            "runtime::steering_canary::workspace::tests::canary_workspace_and_marker_are_private_under_permissive_umask", false,
        ).env("HIVE_CANARY_MODE_FIXTURE", "1"));
        return;
    }
    // SAFETY: only this fixture runs in an owned isolated child process.
    unsafe {
        libc::umask(0);
    }
    let parent = tempfile::tempdir().unwrap();
    let workspace = create(parent.path()).unwrap();
    assert_eq!(
        workspace.path().metadata().unwrap().permissions().mode() & 0o777,
        0o700
    );
    let marker = workspace.path().join("fixed.txt");
    assert_eq!(
        marker.metadata().unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::read(marker).unwrap(),
        b"Hive native steering boundary.\n"
    );
    assert_eq!(fs::read_dir(workspace.path()).unwrap().count(), 1);
}
