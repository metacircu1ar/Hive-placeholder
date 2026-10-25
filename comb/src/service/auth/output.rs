//! Prepared credential publication; no credential exists at the final name until complete.
use super::{Connection, Error, Result, disk};
use std::{os::unix::fs::MetadataExt, path::Path};

pub struct CredentialOutput(atomic_fs::PreparedCreate);
impl CredentialOutput {
    pub fn prepare(path: &Path) -> Result<Self> {
        hive_setup_support::names::check_output_name(path)?;
        if disk::exists(path)? {
            return Err(Error::credential_file_exists());
        }
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        disk::dir(parent, false, &disk::System)?;
        let prepared = atomic_fs::PreparedCreate::new(
            path,
            (hive_setup_support::names::PUBLICATION_OPTIONS).with_prefix(".comb-tmp-"),
        )
        .map(Self)
        .map_err(publication)?;
        if let Err(error) = prepared.check_parent() {
            return Err(with_cleanup(error, prepared.cancel()));
        }
        Ok(prepared)
    }
    pub fn publish(self, connection: &Connection) -> Result<()> {
        if let Err(error) = self.check_parent() {
            return Err(with_cleanup(error, self.cancel()));
        }
        let bytes = match crate::error::json_bytes(connection) {
            Ok(bytes) => bytes,
            Err(error) => return Err(with_cleanup(error, self.cancel())),
        };
        self.0.publish(&bytes).map_err(publication)
    }
    pub fn cancel(self) -> Result<()> {
        self.0.cancel().map_err(publication)
    }
    fn check_parent(&self) -> Result<()> {
        let meta = self.0.parent_metadata()?;
        if !meta.is_dir()
            || meta.uid() != rustix::process::geteuid().as_raw()
            || !hive_setup_support::directories::owned(
                meta.uid(),
                meta.mode(),
                rustix::process::geteuid().as_raw(),
            )
        {
            return Err(Error::unsafe_directory());
        }
        Ok(())
    }
}
fn publication(error: atomic_fs::Error) -> Error {
    let mut native = Error::from(std::io::Error::from(error.io_error().kind()));
    native.file_committed = error.published();
    native.caused_by(error)
}

/// Preserve the primary outcome and both private diagnostics when cleanup fails.
pub fn with_cleanup(primary: Error, cleanup: Result<()>) -> Error {
    match cleanup {
        Ok(()) => primary,
        Err(secondary) => {
            tracing::warn!(name: "comb.cleanup_failed",
                 primary = &primary as &(dyn std::error::Error + 'static),
                 secondary = &secondary as &(dyn std::error::Error + 'static),
                 decision = "preserve_primary_failure");
            let mut error = primary.clone();
            error.file_committed |= secondary.file_committed;
            error.caused_by(Cleanup { primary, secondary })
        }
    }
}
#[derive(Debug)]
struct Cleanup {
    primary: Error,
    secondary: Error,
}
impl std::fmt::Display for Cleanup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cleanup failed: {}",
            self.secondary
                .diagnostic()
                .unwrap_or_else(|| self.secondary.to_string())
        )
    }
}
impl std::error::Error for Cleanup {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.primary)
    }
}

/// Cleanup failure is operational even when the primary preparation error is input.
pub fn has_cleanup_failure(error: &Error) -> bool {
    use std::error::Error as _;
    let mut cause = error.source();
    while let Some(current) = cause {
        if current.is::<Cleanup>()
            || current
                .downcast_ref::<atomic_fs::Error>()
                .is_some_and(|e| e.cleanup_error().is_some())
        {
            return true;
        }
        cause = current.source();
    }
    false
}
