//! Private state files and the public facade for profiles, locks, and recovery.
use crate::{
    error::{Error, Result},
    os,
};
use serde::de::DeserializeOwned;
use std::{
    fs::{self, File},
    io::{self, ErrorKind},
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
pub(crate) const REGISTRATION_INCOMPLETE: &str =
    "Registration is incomplete. Retry auth login for this profile.";

pub(crate) const REGISTER_PROFILE_FIRST: &str = "Register this profile with auth login first.";

mod admission;
mod migration;
pub use admission::MigrationOwnership;
pub use migration::UpgradeReport;
mod controller;
mod format;
mod generations;
mod identity;
mod index;
mod journal;
mod locks;
mod profiles;
mod prune;
mod publication;
mod recovery;

pub use generations::{Generation, GenerationState, KeyAuthority, Repair};
pub(crate) use generations::{is_key_changed, key_changed};
pub use identity::{LockIdentity, ProcessIdentity, boot_identity, same_boot};
pub use locks::Lock;
pub use profiles::{Binding, State};
pub(crate) use publication::{staging_file, write_json, write_json_with_sync};
pub(crate) use recovery::ChildReceipt;
pub(crate) use recovery::ProviderEvidence;
pub use recovery::{
    Operation, OperationKind, RecoveryReport, RecoveryReservation, RuntimeLease, recovery_required,
};

// Maximum profile index or identity record size accepted for parsing.
const STATE_RECORD_BYTES: usize = 1024 * 1024;
// Read one byte beyond the state-record cap to detect oversized records.
const STATE_RECORD_READ_BYTES: u64 = STATE_RECORD_BYTES as u64 + 1;

fn unsafe_state() -> Error {
    Error::unsafe_state(
        "State must be private, owned by this user, and contain no symlinked \
            managed paths.",
    )
}
fn corrupt() -> Error {
    Error::state_inconsistent(
        "Profile index and identity records are inconsistent; no credentials were \
            changed.",
    )
}
pub(crate) fn wall_time() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .inspect_err(|error| {
            crate::observability::observation_failed("wall_clock_before_epoch", error);
        })
        .unwrap_or_default()
}
pub fn now() -> u64 {
    wall_time().as_secs()
}

pub(crate) fn check_path(path: &Path, dir: bool) -> Result<()> {
    checked_metadata(path, dir).map(|_| ())
}

pub(crate) fn checked_metadata(path: &Path, dir: bool) -> Result<fs::Metadata> {
    let meta = fs::symlink_metadata(path).map_err(Error::state_io_from)?;
    check_metadata(&meta, dir)?;
    Ok(meta)
}

/// Validate the same no-follow observation used to read size or inode identity.
pub(crate) fn check_metadata(meta: &fs::Metadata, dir: bool) -> Result<()> {
    if meta.file_type().is_symlink()
        || meta.is_dir() != dir
        || (!dir && !meta.is_file())
        || (!dir && meta.nlink() != 1)
        || meta.uid() != os::effective_uid()
        || meta.mode() & os::PRIVATE_ACCESS_MASK != 0
    {
        return Err(unsafe_state());
    }
    Ok(())
}

pub fn directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => check_path(path, true),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            if let Err(error) = fs::DirBuilder::new()
                .mode(os::PRIVATE_DIRECTORY_MODE)
                .create(path)
                && error.kind() != ErrorKind::AlreadyExists
            {
                return Err(Error::state_io_from(error));
            }
            check_path(path, true)?;
            sync_directory(path.parent().ok_or_else(unsafe_state)?).map_err(Error::state_io_from)
        }
        Err(e) => Err(Error::state_io_from(e)),
    }
}

pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path).and_then(|file| file.sync_all())
}

pub fn private_file(path: &Path, create: bool) -> Result<File> {
    let file = os::private_file(path, create).map_err(Error::state_io_from)?;
    checked_file(file)
}
pub(crate) fn private_reader(path: &Path) -> Result<File> {
    checked_file(os::private_reader(path).map_err(Error::state_io_from)?)
}
fn checked_file(file: File) -> Result<File> {
    let meta = file.metadata().map_err(Error::state_io_from)?;
    if !private_regular(&meta) || meta.nlink() != 1 {
        return Err(unsafe_state());
    }
    Ok(file)
}
fn private_regular(meta: &fs::Metadata) -> bool {
    meta.is_file()
        && meta.uid() == os::effective_uid()
        && meta.mode() & os::PRIVATE_ACCESS_MASK == 0
}
/// Opens of a record that keeps being atomically replaced before they are read.
const REPLACED_RECORD_ATTEMPTS: usize = 4;
/// How a read treats a record that was renamed over after it was opened.
#[derive(Clone, Copy)]
enum Replaced {
    /// An unlocked reader can race its writer: read the replacement.
    Reopen,
    /// A reader that excludes writers never sees one: refuse it.
    Refuse,
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = read_json_bytes(path, Replaced::Refuse)?;
    parse_record(&bytes)
}
fn parse_record<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    // State records have named fields; serde's positional struct arrays are not this format.
    if bytes.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
        return Err(corrupt());
    }
    serde_json::from_slice(bytes).map_err(|cause| corrupt().with_source(cause))
}
fn read_json_bytes(path: &Path, replaced: Replaced) -> Result<Vec<u8>> {
    for _ in 0..REPLACED_RECORD_ATTEMPTS {
        let file = os::private_reader(path).map_err(Error::state_io_from)?;
        #[cfg(test)]
        sync_tests::step(
            path,
            hive_setup_support::checkpoints::Checkpoint::RecordOpened,
        )
        .map_err(Error::state_io_from)?;
        let meta = file.metadata().map_err(Error::state_io_from)?;
        // Unlocked readers can open a record just before a writer renames its
        // replacement over it: the opened inode then has no links. Read the
        // replacement; hard links and foreign or shared files stay refused.
        if matches!(replaced, Replaced::Reopen) && meta.nlink() == 0 && private_regular(&meta) {
            continue;
        }
        if !private_regular(&meta) || meta.nlink() != 1 {
            return Err(unsafe_state());
        }
        let bytes = os::read_capped(file, STATE_RECORD_READ_BYTES).map_err(Error::state_io_from)?;
        if bytes.len() > STATE_RECORD_BYTES {
            return Err(corrupt());
        }
        return Ok(bytes);
    }
    Err(unsafe_state())
}

#[cfg(test)]
fn assert_strict_record<T: serde::de::DeserializeOwned>(value: &serde_json::Value, enums: &[&str]) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("state");
    directory(&root).unwrap();
    let path = root.join("record.json");
    write_json(&path, value).unwrap();
    read_json::<T>(&path).unwrap_or_else(|error| panic!("invalid fixture: {error:?}"));
    for field in value.as_object().unwrap().keys() {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        write_json(&path, &missing).unwrap();
        assert_eq!(
            read_json::<T>(&path).err().unwrap().code(),
            "state_inconsistent",
            "missing {field}"
        );
    }
    let mut unknown = value.clone();
    unknown["unexpected"] = serde_json::json!(true);
    write_json(&path, &unknown).unwrap();
    assert_eq!(
        read_json::<T>(&path).err().unwrap().code(),
        "state_inconsistent"
    );
    for field in enums {
        let mut unknown = value.clone();
        unknown[field] = serde_json::json!("unsupported");
        write_json(&path, &unknown).unwrap();
        assert_eq!(
            read_json::<T>(&path).err().unwrap().code(),
            "state_inconsistent",
            "unknown {field}"
        );
    }
}

#[cfg(test)]
#[path = "state/sync_tests.rs"]
pub(crate) mod sync_tests;

#[cfg(test)]
mod test_support;
