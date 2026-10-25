//! Publish complete administration under the existing external root lock.
use super::{
    Durability, IDENTITY_BYTES, Identity, OwnedLock, atomic, check_file, dir, durable::publication,
    filesystem::same_device, read,
};
use crate::{Error, Result, error::CommitExt, values::CombId};
use atomic_fs::Barriers;
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};
#[cfg(test)]
use {super::durable::checkpoint, atomic_fs::Step};

pub(super) fn create(admin: &Path, durability: &dyn Durability) -> Result<()> {
    atomic_fs::Publisher::new(
        (hive_setup_support::names::PUBLICATION_OPTIONS).with_prefix(".comb-tmp-"),
    )
    .with_barriers(&Initialization {
        #[cfg(test)]
        admin,
        durability,
    })
    .staged_dir_retaining(admin, |stage| {
        prepare(stage, durability).map_err(io::Error::other)
    })
    .map_err(publication)
}

fn prepare(stage: &Path, durability: &dyn Durability) -> Result<()> {
    // Never generate or write a secret until its ignore protection is durable.
    atomic(&stage.join(".gitignore"), b"*\n", durability)?;
    for name in ["heads", "receipts"] {
        let path = stage.join(name);
        dir(&path, true, durability)?;
        #[cfg(test)]
        checkpoint(&path, Step::AfterPublish)?;
    }
    atomic(&stage.join("lock"), b"", durability)?;
    let identity = Identity {
        id: CombId::generate(),
        key: crate::random::bytes()?,
    };
    atomic(
        &stage.join("identity.json"),
        &crate::error::json_bytes(&identity)?,
        durability,
    )?;
    atomic(&stage.join("format"), b"{\"format\":1}\n", durability)
}

pub(super) fn open(
    admin: &Path,
    device: u64,
    durability: &dyn Durability,
) -> Result<(OwnedLock, Identity)> {
    dir(admin, false, durability)?;
    same_device(admin, device)?;
    if fs::metadata(admin)?.mode() & 0o077 != 0 {
        return Err(Error::unsafe_admin_mode());
    }
    check_format(admin)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(admin.join("lock"))?;
    let meta = lock.metadata()?;
    check_file(&meta)?;
    if !meta.is_file() || meta.nlink() != 1 || meta.mode() & 0o077 != 0 {
        return Err(Error::unsafe_lock());
    }
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .map_err(|cause| Error::already_open().caused_by(cause))?;
    let lock = OwnedLock(lock);
    #[cfg(test)]
    if super::INHERIT_LOCK.get() {
        let duplicate = lock.0.try_clone().expect("duplicate the lock descriptor");
        super::INHERITED_LOCK.set(Some(duplicate));
    }
    for name in ["heads", "receipts"] {
        dir(&admin.join(name), false, durability)?;
        same_device(&admin.join(name), device)?;
    }
    let identity = serde_json::from_slice(&read(&admin.join("identity.json"), IDENTITY_BYTES)?)?;
    Ok((lock, identity))
}

/// Normal acquisition only: upgrade inspection intentionally skips this barrier.
pub(super) fn admit_writes(
    admin: &Path,
    lock: &OwnedLock,
    durability: &dyn Durability,
) -> Result<()> {
    let original = fs::symlink_metadata(admin)?;
    let held = lock.0.metadata()?;
    let owner = || {
        let directory = fs::symlink_metadata(admin)?;
        let named = fs::symlink_metadata(admin.join("lock"))?;
        if !directory.is_dir()
            || !named.is_file()
            || (directory.dev(), directory.ino()) != (original.dev(), original.ino())
            || (named.dev(), named.ino()) != (held.dev(), held.ino())
        {
            return Err(hive_setup_support::invalid(
                "Migration admission ownership changed.",
            ));
        }
        Ok(())
    };
    hive_setup_support::migration::admit_writes(
        admin,
        &owner,
        &Initialization {
            #[cfg(test)]
            admin,
            durability,
        },
    )?;
    Ok(())
}

pub(super) fn check_format(admin: &Path) -> Result<()> {
    dir(admin, false, &super::System)?;
    if fs::metadata(admin)?.mode() & 0o077 != 0 {
        return Err(Error::unsafe_admin_mode());
    }
    if super::exists(&admin.join(hive_setup_support::migration::PENDING))? {
        return Err(Error::pending_upgrade());
    }
    if !super::exists(&admin.join("format"))? {
        return Err(Error::unsupported_format());
    }
    let bytes = read(&admin.join("format"), 4096)?;
    if bytes.iter().find(|b| !b.is_ascii_whitespace()) != Some(&b'{') {
        return Err(Error::unsupported_format());
    }
    let marker: hive_input::compatibility::Marker = serde_json::from_slice(&bytes)?;
    if marker.format > crate::compatibility::ROOT.current {
        return Err(Error::newer_format());
    }
    if marker.format != crate::compatibility::ROOT.current {
        return Err(Error::unsupported_format());
    }
    Ok(())
}

pub(super) fn finish_parent(root: &Path, durability: &dyn Durability) -> Result<()> {
    #[cfg(test)]
    checkpoint(root, Step::BeforeDirectorySync).committed()?;
    durability
        .sync_directory(&File::open(root).committed()?)
        .committed()?;
    #[cfg(test)]
    checkpoint(root, Step::AfterDirectorySync).committed()?;
    Ok(())
}

struct Initialization<'a> {
    #[cfg(test)]
    admin: &'a Path,
    durability: &'a dyn Durability,
}
impl Barriers for Initialization<'_> {
    fn sync_file(&self, file: &File) -> io::Result<()> {
        self.durability.fsync(file)
    }
    fn sync_directory(&self, file: &File) -> io::Result<()> {
        self.durability.sync_directory(file)
    }
    #[cfg(test)]
    fn observe(&self, step: Step) -> io::Result<()> {
        checkpoint(self.admin, step)
    }
}

#[cfg(test)]
mod tests;
