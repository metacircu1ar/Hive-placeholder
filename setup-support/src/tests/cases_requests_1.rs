//! Requests regressions.
use super::*;

#[test]
fn metadata_observation_distinguishes_missing_from_unavailable_and_dangling() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing");
    assert!(files::optional_metadata(&missing).unwrap().is_none());
    let dangling = temp.path().join("dangling");
    std::os::unix::fs::symlink("missing", &dangling).unwrap();
    assert!(
        files::optional_metadata(&dangling)
            .unwrap()
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(files::absolute(&dangling).is_err());
    let looping = temp.path().join("loop");
    std::os::unix::fs::symlink("loop", &looping).unwrap();
    let inaccessible = looping.join("child");
    assert_eq!(
        files::optional_metadata(&inaccessible)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::ELOOP)
    );
    assert_eq!(
        files::absolute(&inaccessible).unwrap_err().raw_os_error(),
        Some(libc::ELOOP)
    );
}
