//! Immutable, bounded file capture. Only this module creates trusted attachment metadata.
mod capture;
pub(super) mod delivery;
mod files;
pub(super) mod images;
mod layout;
mod media;
mod removal;
use crate::error::{Error, Result};
pub(super) use capture::Capture;
pub(super) use files::sweep_staging;
#[cfg(test)]
pub(super) use removal::tests::boundaries as removal_boundaries;
pub(super) use removal::{remove_orphan, remove_snapshot};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub(crate) const MAX_FILES: usize = 10;
pub(super) const FILE_BYTES: u64 = 20 * 1024 * 1024;
pub(super) const MESSAGE_BYTES: u64 = 50 * 1024 * 1024;
pub(super) const NAME_BYTES: usize = 255;
pub(super) type Guard = Arc<tokio::sync::OwnedMutexGuard<()>>;

#[derive(Clone)]
pub(super) struct Snapshot {
    pub key: String,
    pub filename: String,
}
#[derive(Clone)]
pub(super) struct Publication {
    pub root: PathBuf,
    pub files: Vec<Snapshot>,
    pub guard: Guard,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Attachment {
    pub delivery: Delivery,
    pub filename: String,
    pub bytes: u64,
    pub media_type: String,
    pub dimensions: Option<[u32; 2]>,
    pub source: Source,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Delivery {
    Pending,
    NativeImage,
    Reference { reason: ReferenceReason },
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReferenceReason {
    NotImage,
    LivePath,
    Steering,
    ImageDimensions,
    ImageBytes,
    MessageImageBudget,
    ConversationImageBudget,
    InvalidImage,
    FrameBudget,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Source {
    Snapshot {
        key: String,
        sha256: String,
        path: PathBuf,
    },
    LivePath {
        path: PathBuf,
        observed: Identity,
        reason: LiveReason,
    },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LiveReason {
    FileSizeLimit,
    MessageSizeLimit,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    pub device: u64,
    pub inode: u64,
    pub bytes: u64,
    pub mtime_seconds: i64,
    pub mtime_nanoseconds: i64,
    pub ctime_seconds: i64,
    pub ctime_nanoseconds: i64,
}
impl Attachment {
    pub(super) fn validate(&self, root: &Path) -> Result<()> {
        if !valid_name(&self.filename) {
            return Err(Error::state_inconsistent("Invalid attachment filename."));
        }
        match &self.source {
            Source::Snapshot { key, sha256, path } => {
                if !valid_key(sha256)
                    || *key != key_from_digest(sha256, &self.filename)?
                    || self.bytes > FILE_BYTES
                    || *path != snapshot_path(root, key, &self.filename)?
                {
                    return Err(Error::state_inconsistent(
                        "Invalid snapshot attachment metadata.",
                    ));
                }
            }
            Source::LivePath { path, observed, .. } => {
                if !path.is_absolute() || self.bytes != observed.bytes {
                    return Err(Error::state_inconsistent(
                        "Invalid live attachment metadata.",
                    ));
                }
            }
        }
        Ok(())
    }
    pub(super) fn key(&self) -> Option<&str> {
        match &self.source {
            Source::Snapshot { key, .. } => Some(key),
            Source::LivePath { .. } => None,
        }
    }
}
fn key_from_digest(digest: &str, name: &str) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&digest[index * 2..index * 2 + 2], 16).map_err(|cause| {
            Error::state_inconsistent("Invalid attachment digest.").with_source(cause)
        })?;
    }
    let mut hash = Sha256::new();
    hash.update(bytes);
    hash.update(name.as_bytes());
    Ok(format!("{:x}", hash.finalize()))
}
pub(super) fn invalid(path: &Path, reason: &str) -> Error {
    Error::AttachmentInvalid {
        message: format!(
            "Attachment {}: {reason}",
            serde_json::to_string(&path.to_string_lossy()).unwrap_or_default()
        )
        .into(),
        source: None,
    }
}
pub(super) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_BYTES
        && !matches!(name, "." | "..")
        && !name.contains(['/', '\0'])
}
pub(super) fn valid_key(key: &str) -> bool {
    key.len() == 64
        && key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn snapshot_path(root: &Path, key: &str, name: &str) -> Result<PathBuf> {
    if !valid_key(key) || !valid_name(name) {
        return Err(Error::unsafe_state("Invalid stored attachment identity."));
    }
    Ok(root.join(key).join(name))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod native_tests;
