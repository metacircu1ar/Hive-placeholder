//! Copy and fingerprint retained database files without opening the source with SQLite.
// SHA-256's fixed digest width and the bounded streaming read buffer.
use super::{check_path, corrupt};
use crate::{
    error::{Error, Result},
    os,
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};
const SHA256_BYTES: usize = 32;
const HASH_READ_BYTES: usize = 8192;
// The snapshot entries are database, WAL, then rollback journal, in that order.
const JOURNAL_INDEX: usize = 2;

#[derive(PartialEq, Eq)]
struct FileTimestamp {
    seconds: i64,
    nanoseconds: i64,
}
#[derive(PartialEq, Eq)]
pub(super) struct Fingerprint {
    dev: u64,
    ino: u64,
    len: u64,
    modified: FileTimestamp,
    changed: FileTimestamp,
    digest: [u8; SHA256_BYTES],
}
fn content_digest(path: &Path) -> Result<Option<[u8; SHA256_BYTES]>> {
    if !super::recovery::exists(path)? {
        return Ok(None);
    }
    let mut file = super::private_reader(path)?;
    let mut hash = Sha256::new();
    let mut bytes = [0; HASH_READ_BYTES];
    loop {
        let n = file.read(&mut bytes).map_err(Error::state_io_from)?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    Ok(Some(hash.finalize().into()))
}
pub(super) fn fingerprint(path: &Path) -> Result<Option<Fingerprint>> {
    let Some(digest) = content_digest(path)? else {
        return Ok(None);
    };
    let m = fs::symlink_metadata(path).map_err(Error::state_io_from)?;
    Ok(Some(Fingerprint {
        dev: m.dev(),
        ino: m.ino(),
        len: m.len(),
        modified: FileTimestamp {
            seconds: m.mtime(),
            nanoseconds: m.mtime_nsec(),
        },
        changed: FileTimestamp {
            seconds: m.ctime(),
            nanoseconds: m.ctime_nsec(),
        },
        digest,
    }))
}

/// Copy a stable database and WAL without letting SQLite open the source.
pub(super) fn snapshot_database(path: &Path) -> Result<tempfile::TempDir> {
    let snapshot = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(os::PRIVATE_DIRECTORY_MODE))
        .tempdir()
        .map_err(Error::state_io_from)?;
    crate::publication::prepare(snapshot, |directory| {
        copy_snapshot(path, directory, || Ok(()))
    })
    .map(|(directory, ())| directory)
}

fn copy_snapshot(path: &Path, snapshot: &Path, copied: impl FnOnce() -> Result<()>) -> Result<()> {
    let runtime = path.parent().ok_or_else(corrupt)?;
    check_path(runtime, true)?;
    let filename = path.file_name().ok_or_else(corrupt)?;
    let mut wal = filename.to_os_string();
    wal.push("-wal");
    let mut journal = filename.to_os_string();
    journal.push("-journal");
    let names = [filename, wal.as_os_str(), journal.as_os_str()];
    let before = names
        .iter()
        .map(|n| fingerprint(&runtime.join(n)))
        .collect::<Result<Vec<_>>>()?;
    if before[JOURNAL_INDEX]
        .as_ref()
        .is_some_and(|file| file.len != 0)
    {
        return Err(Error::recovery_required(concat!(
            "Runtime history has an unresolved rollback journal; retain it and use a ",
            "fresh state directory.",
        )));
    }
    for (name, present) in names.iter().zip(&before) {
        if present.is_some() {
            let mut from = super::private_reader(&runtime.join(name))?;
            atomic_fs::PreparedCreate::new(
                snapshot.join(name),
                (hive_setup_support::names::PUBLICATION_OPTIONS)
                    .with_durability(atomic_fs::Durability::AtomicVisible),
            )
            .and_then(|prepared| {
                prepared.publish_stream(
                    |output| io::copy(&mut from, output),
                    &atomic_fs::DefaultBarriers,
                )
            })
            .map_err(crate::publication::error)?;
        }
    }
    copied()?;
    let after = names
        .iter()
        .map(|n| fingerprint(&runtime.join(n)))
        .collect::<Result<Vec<_>>>()?;
    let copied = names
        .iter()
        .map(|n| content_digest(&snapshot.join(n)))
        .collect::<Result<Vec<_>>>()?;
    if before != after
        || before
            .iter()
            .map(|f| f.as_ref().map(|f| f.digest))
            .collect::<Vec<_>>()
            != copied
    {
        return Err(Error::recovery_required(concat!(
            "Runtime history changed during inspection; retry when it is idle. No ",
            "source files or locks were modified.",
        )));
    }
    if before[0].is_none() {
        return Err(Error::invalid_request("Runtime database is missing."));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
