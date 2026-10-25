//! A durable external permit survives deletion of the probe's ownership evidence.
use super::{Error, Lock, LockIdentity, Result, Uuid, check_path, corrupt, exists, read_json};
use crate::{
    publication,
    state::{self, journal},
};
use atomic_fs::{Barriers, DirectoryIdentity, Step};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    device: u64,
    inode: u64,
}
impl Identity {
    pub(super) fn at(path: &Path) -> Result<Self> {
        check_path(path, true)?;
        let metadata = fs::symlink_metadata(path).map_err(Error::state_io_from)?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    pub(super) fn check(self, path: &Path) -> Result<()> {
        if Self::at(path)? != self {
            return Err(corrupt());
        }
        Ok(())
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Permit {
    id: Uuid,
    parent: Identity,
    tree: Identity,
    credential_lock: LockIdentity,
}
pub(super) struct Paths {
    id: Uuid,
    original: PathBuf,
    aside: PathBuf,
    permit: PathBuf,
    parent: PathBuf,
}
impl Paths {
    pub(super) fn new(dir: &Path) -> Result<Self> {
        let id = canonical(dir.file_name().ok_or_else(corrupt)?).ok_or_else(corrupt)?;
        let parent = dir.parent().ok_or_else(corrupt)?.to_path_buf();
        check_path(&parent, true)?;
        Ok(Self {
            id,
            original: dir.into(),
            aside: parent.join(format!("{id}.pruning")),
            permit: parent.join(format!("{id}.prune.json")),
            parent,
        })
    }
    pub(super) fn allocate(&self) -> Result<()> {
        if exists(&self.original)? || exists(&self.aside)? || exists(&self.permit)? {
            return Err(corrupt());
        }
        state::directory(&self.original)
    }
    pub(super) fn permitted(&self) -> Result<bool> {
        if exists(&self.permit)? {
            self.load()?;
            Ok(true)
        } else if exists(&self.aside)? {
            Err(corrupt())
        } else {
            Ok(false)
        }
    }
    pub(super) fn permit(&self, lock: &Lock) -> Result<()> {
        if exists(&self.aside)? || exists(&self.permit)? {
            return Err(corrupt());
        }
        let permit = Permit {
            id: self.id,
            parent: Identity::at(&self.parent)?,
            tree: Identity::at(&self.original)?,
            credential_lock: LockIdentity::from_fd(lock.fd())?,
        };
        if LockIdentity::at(&self.original.join("auth.lock"))? != permit.credential_lock {
            return Err(corrupt());
        }
        journal::publish(&self.permit, &permit)
    }
    fn load(&self) -> Result<Permit> {
        let permit: Permit = read_json(&self.permit)?;
        if permit.id != self.id {
            return Err(corrupt());
        }
        permit.parent.check(&self.parent)?;
        match (exists(&self.original)?, exists(&self.aside)?) {
            (true, false) => {
                permit.tree.check(&self.original)?;
                // Before rename the original ownership marker must still be intact.
                if LockIdentity::at(&self.original.join("auth.lock"))? != permit.credential_lock {
                    return Err(corrupt());
                }
            }
            (false, true) => permit.tree.check(&self.aside)?,
            (false, false) => {}
            _ => return Err(corrupt()),
        }
        Ok(permit)
    }
    pub(super) fn remove(&self) -> Result<()> {
        let permit = self.load()?;
        journal::sync_record(&self.permit)?;
        permit.parent.check(&self.parent)?;
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .with_barriers(&journal::Boundary(&self.original))
            .remove_named_dir(
                &self.original,
                &self.aside,
                DirectoryIdentity::new(permit.tree.device, permit.tree.inode),
            )
            .map_err(publication::error)?;
        let boundary = journal::Boundary(&self.permit);
        boundary
            .observe(Step::BeforeRemove)
            .map_err(Error::state_io_from)?;
        permit.parent.check(&self.parent)?;
        self.load()?;
        fs::remove_file(&self.permit).map_err(Error::state_io_from)?;
        boundary
            .observe(Step::AfterRemove)
            .map_err(Error::state_io_from)?;
        boundary.sync_parent()
    }
}
fn canonical(name: &OsStr) -> Option<Uuid> {
    let name = name.to_str()?;
    let id = Uuid::parse_str(name).ok()?;
    (id.to_string() == name).then_some(id)
}
pub(super) fn entry_id(name: &OsStr) -> Option<Uuid> {
    let name = name.to_str()?;
    canonical(OsStr::new(
        name.strip_suffix(".prune.json")
            .or_else(|| name.strip_suffix(".pruning"))
            .unwrap_or(name),
    ))
}

#[cfg(test)]
#[test]
fn permits_require_exact_fields() {
    state::assert_strict_record::<Permit>(
        &serde_json::json!({
            "id": Uuid::nil(), "parent": {"device":1,"inode":2},
            "tree": {"device":1,"inode":3}, "credential_lock":{"dev":1,"ino":4},
        }),
        &[],
    );
    state::assert_strict_record::<Identity>(&serde_json::json!({"device":1,"inode":2}), &[]);
}
