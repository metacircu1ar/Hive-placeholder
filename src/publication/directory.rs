//! Disarm path-based recursive cleanup when the owned directory identity changes.
use crate::{
    error::{Error, Result},
    os,
};
use std::{fs, os::unix::fs::MetadataExt};
use tempfile::TempDir;

pub(super) struct Owned {
    directory: Option<TempDir>,
    identity: fs::Metadata,
}
impl Owned {
    pub(super) fn new(directory: TempDir) -> Result<Self> {
        let identity = fs::symlink_metadata(directory.path())
            .map_err(Error::state_io_from)
            .and_then(|metadata| {
                private(&metadata)?;
                Ok(metadata)
            });
        match identity {
            Ok(identity) => Ok(Self {
                directory: Some(directory),
                identity,
            }),
            Err(error) => {
                let _retained = directory.keep();
                Err(error)
            }
        }
    }
    fn check(&self) -> Result<()> {
        if let Some(directory) = &self.directory {
            let actual = fs::symlink_metadata(directory.path()).map_err(Error::state_io_from)?;
            private(&actual)?;
            if actual.dev() != self.identity.dev() || actual.ino() != self.identity.ino() {
                return Err(Error::unsafe_state(
                    "Private preparation directory changed.",
                ));
            }
        }
        Ok(())
    }
    pub(super) fn retain(&mut self) {
        if let Some(directory) = self.directory.take() {
            let _retained = directory.keep();
        }
    }
    pub(super) fn release(mut self) -> Result<TempDir> {
        self.check()?;
        self.directory
            .take()
            .ok_or_else(|| Error::internal_error("Preparation directory is unavailable."))
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        // A nested reservation's destructor cannot return its cleanup evidence
        // during unwind. Retain the whole directory rather than erase that evidence.
        if std::thread::panicking() {
            self.retain();
        } else if let Err(error) = self.check() {
            tracing::warn!(name: "publication.cleanup_refused",
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "retain_evidence");
            self.retain();
        }
    }
}
fn private(metadata: &fs::Metadata) -> Result<()> {
    if !metadata.is_dir()
        || metadata.uid() != os::effective_uid()
        || metadata.mode() & os::PRIVATE_ACCESS_MASK != 0
    {
        return Err(Error::unsafe_state(
            "Preparation directory must be private and owned.",
        ));
    }
    Ok(())
}
