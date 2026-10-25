//! Strict ownership evidence; connection outcomes are never evidence.
use super::files::{Parent, native, private};
use crate::{Error, Result, disk::Disk, values::CombId};
use rustix::fs::{self, AtFlags, Mode, OFlags, Stat};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Node {
    device: i128,
    inode: i128,
}
impl Node {
    pub fn matches_metadata(&self, stat: &std::fs::Metadata) -> bool {
        use std::os::unix::fs::MetadataExt;
        self.device == i128::from(stat.dev()) && self.inode == i128::from(stat.ino())
    }
    pub fn of(stat: &Stat) -> Self {
        Self {
            device: stat.st_dev.into(),
            inode: stat.st_ino.into(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    node: Node,
    ctime: i128,
    ctime_nsec: i128,
}
impl Entry {
    pub fn socket(stat: &Stat) -> Result<Self> {
        if stat.st_mode & libc::S_IFMT != libc::S_IFSOCK
            || stat.st_nlink != 1
            || stat.st_uid != rustix::process::geteuid().as_raw()
            || stat.st_mode & 0o077 != 0
        {
            return Err(Error::unsafe_socket());
        }
        Ok(Self {
            node: Node::of(stat),
            ctime: stat.st_ctime.into(),
            ctime_nsec: stat.st_ctime_nsec.into(),
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Root {
    pub path: PathBuf,
    node: Node,
    id: CombId,
}
impl Root {
    pub fn read(path: &Path) -> Result<Self> {
        let path = path.canonicalize()?;
        let before = fs::statat(fs::CWD, &path, AtFlags::SYMLINK_NOFOLLOW).map_err(native)?;
        let (id, _) = Disk::human_authority(&path)?;
        let root = Self {
            path,
            node: Node::of(&before),
            id,
        };
        root.check()?;
        Ok(root)
    }
    pub fn check(&self) -> Result<()> {
        let stat = fs::statat(fs::CWD, &self.path, AtFlags::SYMLINK_NOFOLLOW).map_err(native)?;
        if stat.st_mode & libc::S_IFMT != libc::S_IFDIR
            || Node::of(&stat) != self.node
            || stat.st_uid != rustix::process::geteuid().as_raw()
            || !hive_setup_support::directories::owned(
                stat.st_uid,
                stat.st_mode,
                rustix::process::geteuid().as_raw(),
            )
            || Disk::human_authority(&self.path)?.0 != self.id
        {
            return Err(Error::socket_in_use());
        }
        Ok(())
    }
    pub fn check_disk(&self, disk: &Disk) -> Result<()> {
        self.check()?;
        if disk.root != self.path || disk.id() != self.id {
            return Err(Error::socket_in_use());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt {
    pub path: PathBuf,
    pub parent: Node,
    pub root: Root,
    pub entry: Entry,
}
pub(super) fn read(parent: &Parent, name: &str) -> Result<Option<Receipt>> {
    let file = match fs::openat(
        &parent.file,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => File::from(fd),
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(error) => return Err(native(error)),
    };
    let stat = fs::fstat(&file).map_err(native)?;
    private(&stat)?;
    let bytes = crate::bounded::bytes(file, 16 * 1024, Error::file_size_limit)?;
    if parent.stat(name.as_ref())?.as_ref().map(Node::of) != Some(Node::of(&stat)) {
        return Err(Error::socket_in_use());
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    if !value.is_object() || has_array(&value) {
        return Err(Error::socket_in_use());
    }
    // Decode the original bytes: Value projection would collapse duplicate fields.
    Ok(Some(serde_json::from_slice(&bytes)?))
}

fn has_array(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(_) => true,
        serde_json::Value::Object(fields) => fields.values().any(has_array),
        _ => false,
    }
}
