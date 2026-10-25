//! Create isolated controller roots and preserve retired credential namespace fences.
#[cfg(test)]
use super::write_json;
use super::{State, check_path, corrupt, private_reader, read_json, recovery};
use crate::{
    config::Provider,
    error::{Error, Result},
    os,
};
#[cfg(test)]
use export::export_path;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
const SEPARATE_CONTROLLER: &str =
    "Replacement controller must be physically separate from the retired root.";

mod export;
mod publication;
mod queries;
mod snapshot;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetiredController {
    pub root: PathBuf,
    pub claude_namespaces: Vec<String>,
    #[serde(deserialize_with = "serde::Deserialize::deserialize")]
    pub metadata_unavailable: Option<String>,
}
impl State {
    pub(crate) fn replacement_destination(&self, destination: &Path) -> Result<PathBuf> {
        // Isolation needs the old root's identity, not readable contents. It never
        // mutates that root; replacement_metadata fences unknown Claude namespaces.
        let old = self.root.canonicalize().map_err(Error::state_io_from)?;
        check_path(&old, true)?;
        if !destination.is_absolute() || super::recovery::exists(destination)? {
            return Err(Error::invalid_path(
                "Replacement state directory must be a new absolute path.",
            ));
        }
        let parent = destination
            .parent()
            .ok_or_else(corrupt)?
            .canonicalize()
            .map_err(Error::state_io_from)?;
        let destination = parent.join(destination.file_name().ok_or_else(corrupt)?);
        if destination.starts_with(&old) || old.starts_with(&destination) {
            return Err(Error::invalid_path(SEPARATE_CONTROLLER));
        }
        let old_identity = fs::metadata(&old).map_err(Error::state_io_from)?;
        for ancestor in parent.ancestors() {
            let identity = fs::metadata(ancestor).map_err(Error::state_io_from)?;
            if (identity.dev(), identity.ino()) == (old_identity.dev(), old_identity.ino()) {
                return Err(Error::invalid_path(SEPARATE_CONTROLLER));
            }
        }
        Ok(destination)
    }
    pub fn replacement_root(&self, destination: &Path, provider: Provider) -> Result<State> {
        publication::create(self, destination, provider, &atomic_fs::DefaultBarriers)
    }
    pub(crate) fn replacement_metadata(&self, provider: Provider) -> Result<RetiredController> {
        let old = self.root.canonicalize().map_err(Error::state_io_from)?;
        // Read metadata only: no old locks, writable SQLite opens or native calls.
        // Unreadable namespace metadata needs filesystem repair; never guess at
        // a Keychain namespace that an unresolved old writer could still use.
        let snapshot = self
            .migration_index()
            .and_then(|index| self.retired_namespaces().map(|names| (index, names)));
        if let Err(error) = &snapshot
            && provider == Provider::Claude
        {
            return Err(Error::recovery_required(concat!(
                "Old credential namespace metadata is unreadable. Claude replacement ",
                "needs filesystem repair of that metadata to exclude Keychain collisions.",
                " Codex can use auth repair --reauthenticate --new-state-dir with a fresh",
                " root. No reboot is required.",
            ))
            .with_source(error.clone()));
        }
        let metadata_unavailable = snapshot.as_ref().err().map(ToString::to_string);
        let namespaces = match snapshot {
            Ok((index, mut names)) => {
                names.extend(index.claude_namespaces(None).collect::<Result<Vec<_>>>()?);
                names
            }
            Err(error) => {
                crate::observability::observation_failed("retired_controller_metadata", &error);
                Vec::new()
            }
        };
        Ok(RetiredController {
            root: old,
            claude_namespaces: namespaces,
            metadata_unavailable,
        })
    }
    pub(super) fn retired_namespaces(&self) -> Result<Vec<String>> {
        let path = self.root.join("retired-controller.json");
        if !super::recovery::exists(&path)? {
            return Ok(Vec::new());
        }
        let retired: RetiredController = read_json(&path)?;
        if !retired.root.is_absolute() {
            return Err(corrupt());
        }
        if retired.metadata_unavailable.is_some() {
            return Err(Error::recovery_required(concat!(
                "Retired Claude namespace metadata is unavailable. This controller can ",
                "serve Codex. After repairing the original root's metadata, run auth ",
                "repair --reauthenticate --new-state-dir from that original root to ",
                "create another controller for Claude. This controller's saved exclusion ",
                "fence is retained.",
            )));
        }
        Ok(retired.claude_namespaces)
    }
    pub fn retired_controller(&self) -> Result<Option<PathBuf>> {
        let path = self.root.join("retired-controller.json");
        if !super::recovery::exists(&path)? {
            return Ok(None);
        }
        let retired: RetiredController = read_json(&path)?;
        if !retired.root.is_absolute() {
            return Err(corrupt());
        }
        Ok(Some(retired.root))
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod corrupt_source_tests;

#[cfg(test)]
#[test]
fn retired_controller_requires_exact_fields() {
    super::assert_strict_record::<RetiredController>(
        &serde_json::json!({"root": "/retained",
             "claude_namespaces": [],
             "metadata_unavailable": null}),
        &[],
    );
}
