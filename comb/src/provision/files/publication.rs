//! Bounded model/setup records using the common atomic publication engine.
use super::{RECORD_BYTES, directory, invalid, open, owned};
use crate::{Result, error::publication};
use atomic_fs::{Barriers, PreparedCreate, Step};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    cell::RefCell,
    fs::{self, File},
    io::{self, Read},
    os::unix::fs::MetadataExt,
    path::Path,
};

pub(crate) fn encode_record(value: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec_pretty(value)?;
    if bytes.len() as u64 > RECORD_BYTES {
        return Err(invalid("Model file exceeds its size bound."));
    }
    Ok(bytes)
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    write_json_hook(path, value, &mut |_| Ok(()))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Publish {
    Prepared,
    Renamed,
}

pub(crate) fn write_json_hook(
    path: &Path,
    value: &impl Serialize,
    hook: &mut impl FnMut(Publish) -> Result<()>,
) -> Result<()> {
    publish(path, &encode_record(value)?, false, hook)
}

pub(crate) fn create_record(path: &Path, bytes: &[u8]) -> Result<()> {
    publish(path, bytes, true, &mut |_| Ok(()))
}

fn publish(
    path: &Path,
    bytes: &[u8],
    once: bool,
    hook: &mut dyn FnMut(Publish) -> Result<()>,
) -> Result<()> {
    if bytes.len() as u64 > RECORD_BYTES {
        return Err(invalid("Model file exceeds its size bound."));
    }
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Missing model directory."))?;
    directory(parent)?;
    match path.symlink_metadata() {
        Ok(_) => owned(path, false)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    let barriers = Publication {
        path,
        hook: RefCell::new(hook),
    };
    let options = (hive_setup_support::names::PUBLICATION_OPTIONS).with_prefix(".comb-tmp-");
    if once {
        PreparedCreate::new(path, options)
            .and_then(|prepared| prepared.publish_with(bytes, &barriers))
    } else {
        atomic_fs::Publisher::new(options)
            .with_barriers(&barriers)
            .write(path, bytes)
    }
    .map_err(publication)
}

/// Recheck semantic content on the same descriptor whose durability is repeated.
pub(crate) fn sync_json<T: DeserializeOwned + PartialEq>(path: &Path, expected: &T) -> Result<()> {
    resync(path, expected).map_err(crate::Error::committed)
}
fn resync<T: DeserializeOwned + PartialEq>(path: &Path, expected: &T) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Missing model directory."))?;
    directory(parent)?;
    let held_parent = File::open(parent)?;
    let file = open(path)?;
    let mut bytes = Vec::new();
    (&file).take(RECORD_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > RECORD_BYTES || &serde_json::from_slice::<T>(&bytes)? != expected {
        return Err(invalid("Setup intent changed during publication retry."));
    }
    let mut hook = |_| Ok(());
    let barriers = Publication {
        path,
        hook: RefCell::new(&mut hook),
    };
    let sync = || -> io::Result<()> {
        same_name(&held_parent, parent)?;
        same_name(&file, path)?;
        barriers.observe(Step::BeforeFileSync)?;
        barriers.sync_file(&file)?;
        barriers.observe(Step::AfterFileSync)?;
        same_name(&file, path)?;
        barriers.observe(Step::BeforeDirectorySync)?;
        barriers.sync_directory(&held_parent)?;
        barriers.observe(Step::AfterDirectorySync)?;
        same_name(&held_parent, parent)?;
        same_name(&file, path)
    };
    sync().map_err(|source| publication(atomic_fs::Error::new(source, true)))
}
fn same_name(file: &File, path: &Path) -> io::Result<()> {
    let held = file.metadata()?;
    let named = fs::symlink_metadata(path)?;
    if named.file_type().is_symlink()
        || (held.dev(), held.ino()) != (named.dev(), named.ino())
        || held.uid() != rustix::process::geteuid().as_raw()
        || if held.is_dir() {
            !hive_setup_support::directories::owned(
                held.uid(),
                held.mode(),
                rustix::process::geteuid().as_raw(),
            )
        } else {
            held.mode() & 0o022 != 0
        }
        || (held.is_file() && held.nlink() != 1)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Record publication identity changed",
        ));
    }
    Ok(())
}

struct Publication<'a> {
    path: &'a Path,
    hook: RefCell<&'a mut dyn FnMut(Publish) -> Result<()>>,
}
impl Barriers for Publication<'_> {
    fn observe(&self, step: Step) -> io::Result<()> {
        #[cfg(not(test))]
        let _ = self.path;
        #[cfg(test)]
        hooks::step(self.path, step)?;
        let phase = match step {
            Step::AfterFileSync => Publish::Prepared,
            Step::AfterPublish => Publish::Renamed,
            _ => return Ok(()),
        };
        (self.hook.borrow_mut())(phase).map_err(io::Error::other)
    }
}

#[cfg(test)]
pub(crate) mod hooks;
#[cfg(test)]
mod tests;
