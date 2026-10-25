use super::*;

#[test]
fn isolated_success_rejects_a_nonexistent_selector() {
    let output = isolated_output(&mut rerun_isolated(
        "test_support::tests::nonexistent_isolated_fixture",
        false,
    ));
    // Libtest considers an unmatched filter successful; the runner must not.
    assert!(output.status.success());
    assert!(output.stdout.contains("test result: ok. 0 passed;"));
    assert!(std::panic::catch_unwind(|| output.assert_success()).is_err());
}

#[test]
fn isolated_success_requires_execution_of_regular_and_ignored_tests() {
    run_isolated(&mut rerun_isolated(
        "test_support::tests::isolated_success_rejects_a_nonexistent_selector",
        false,
    ));
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("executed");
    let fixture = "test_support::tests::isolated_marker_fixture";
    let skipped =
        isolated_output(rerun_isolated(fixture, false).env("HIVE_ISOLATED_TEST_MARKER", &marker));
    assert!(skipped.status.success());
    assert!(!marker.exists());
    assert!(std::panic::catch_unwind(|| skipped.assert_success()).is_err());
    run_isolated(rerun_isolated(fixture, true).env("HIVE_ISOLATED_TEST_MARKER", &marker));
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "executed");
}

#[test]
#[ignore = "subprocess fixture"]
fn isolated_marker_fixture() {
    let marker =
        std::env::var_os("HIVE_ISOLATED_TEST_MARKER").expect("isolated marker fixture path");
    std::fs::write(marker, "executed").unwrap();
}
