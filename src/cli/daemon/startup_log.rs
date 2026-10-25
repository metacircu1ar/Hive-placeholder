//! Raw inherited process output. `O_APPEND` preserves bytes across competing starts;
//! unlike structured logs, arbitrary child stdout/stderr is not a record protocol.
use crate::{
    error::{Error, Result},
    os, state,
};
use std::{fs::File, io::Write, path::Path};

pub(super) fn open(path: &Path) -> Result<File> {
    let mut file = state::private_file(path, true)?;
    os::append_mode(&file).map_err(Error::state_io_from)?;
    let separator = b"\n--- Hive daemon startup ---\n";
    if file.write(separator).map_err(Error::state_io_from)? != separator.len() {
        return Err(Error::state_io_from(std::io::Error::new(
            std::io::ErrorKind::WriteZero,
            "Short daemon startup separator",
        )));
    }
    file.sync_all().map_err(Error::state_io_from)?;
    state::sync_directory(
        path.parent()
            .ok_or_else(|| Error::invalid_path("Missing startup log parent."))?,
    )
    .map_err(Error::state_io_from)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn restarts_and_inherited_handles_preserve_existing_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("daemon.log");
        let mut first = open(&path).unwrap();
        first.write_all(b"first process\n").unwrap();
        let prefix = std::fs::read(&path).unwrap();
        let second = open(&path).unwrap();
        first.write_all(b"late first output\n").unwrap();
        let status = std::process::Command::new("/bin/sh")
            .args(["-c", "printf 'stdout\\n'; printf 'stderr\\n' >&2"])
            .stdout(second.try_clone().unwrap())
            .stderr(second)
            .status()
            .unwrap();
        assert!(status.success());
        let data = std::fs::read(&path).unwrap();
        assert!(data.starts_with(&prefix));
        assert!(data.ends_with(b"late first output\nstdout\nstderr\n"));
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn private_checks_precede_startup_output() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("daemon.log");
        std::fs::write(&path, b"foreign").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(open(&path).is_err());
        let link = directory.path().join("link");
        symlink(&path, &link).unwrap();
        assert!(open(&link).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"foreign");
    }
}
