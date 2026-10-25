//! Pin helper launches to the image this process started from, across upgrades.
use crate::error::{Error, Result};
use std::{path::PathBuf, sync::OnceLock};

static IMAGE: OnceLock<Result<PathBuf>> = OnceLock::new();

pub(crate) fn capture() {
    // Cache the complete result, including its native cause, for path() to return.
    let _ = IMAGE.get_or_init(|| {
        std::env::current_exe()
            .and_then(|path| path.canonicalize())
            .map_err(Error::state_io_from)
    });
}

pub(crate) fn path() -> Result<&'static std::path::Path> {
    capture();
    match IMAGE.get() {
        Some(Ok(path)) => Ok(path),
        Some(Err(error)) => Err(error.clone()),
        None => unreachable!("capture initializes IMAGE"),
    }
}

/// Hash a stable opened executable; reject a concurrent replacement or edit.
pub(crate) fn sha256(path: &std::path::Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let before = crate::runtime::provider_binary_identity(path)?;
    let mut input = std::fs::File::open(path).map_err(Error::state_io_from)?;
    let mut hash = Sha256::new();
    std::io::copy(&mut input, &mut hash).map_err(Error::state_io_from)?;
    if !before.unchanged() {
        return Err(Error::daemon_binary_mismatch(
            "Executable changed while its identity was read.",
        ));
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    #[test]
    fn process_keeps_image_after_invocation_link_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let link = tmp.path().join("hivectl");
        let original = std::env::current_exe().unwrap();
        symlink(&original, &link).unwrap();
        let child =
            crate::test_support::rerun_isolated("executable::tests::pinned_image_child", true);
        // Execute the test process through a mutable installation-style link.
        let mut command = std::process::Command::new(&link);
        command
            .args(child.get_args())
            .env("HIVE_PIN_TEST_LINK", &link);
        crate::test_support::run_isolated(&mut command);
    }

    #[test]
    #[ignore = "isolated executable pinning fixture"]
    fn pinned_image_child() {
        capture();
        let before = path().unwrap().to_owned();
        let link = PathBuf::from(std::env::var_os("HIVE_PIN_TEST_LINK").unwrap());
        fs::remove_file(&link).unwrap();
        symlink("/does/not/exist", &link).unwrap();
        assert_eq!(path().unwrap(), before);
        assert!(before.is_file());
        assert_ne!(path().unwrap(), link);
    }
}
