//! Flat images keep their ownership marker until payload deletion is durable.
use crate::{
    error::{Error, Result},
    state,
};
use atomic_fs::{Barriers, Step};
use std::{
    ffi::OsString,
    fs, io,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
};
mod names;
pub(super) use names::{Retirement, image_name, is_aside, matching_asides};

pub(super) struct Boundary<'a>(pub &'a Path);
impl Barriers for Boundary<'_> {
    #[cfg(test)]
    fn observe(&self, step: Step) -> io::Result<()> {
        state::sync_tests::step(self.0, step)
    }
}
impl Boundary<'_> {
    pub(super) fn sync(&self) -> Result<()> {
        state::check_path(self.0, true)?;
        self.observe(Step::BeforeDirectorySync)
            .and_then(|()| self.sync_directory(&fs::File::open(self.0)?))
            .and_then(|()| self.observe(Step::AfterDirectorySync))
            .map_err(Error::state_io_from)
    }
}
pub(super) fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(Error::state_io_from(e)),
    }
}
fn invalid() -> Error {
    Error::unsafe_state("Launcher retirement ownership or layout changed.")
}
fn owner_at(path: &Path, owner: &state::Lock) -> Result<()> {
    if state::LockIdentity::from_fd(owner.fd())?
        != state::LockIdentity::from_fd(state::private_file(path, false)?)?
    {
        return Err(invalid());
    }
    Ok(())
}
#[derive(PartialEq, Eq)]
struct Payload {
    name: OsString,
    identity: state::LockIdentity,
}
impl Payload {
    fn check(&self, dir: &Path) -> Result<()> {
        if state::LockIdentity::from_fd(state::private_reader(&dir.join(&self.name))?)?
            != self.identity
        {
            return Err(invalid());
        }
        Ok(())
    }
}
fn payloads(dir: &Path) -> Result<Vec<Payload>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir).map_err(Error::state_io_from)? {
        let entry = entry.map_err(Error::state_io_from)?;
        if !entry.file_name().to_str().is_some_and(super::owned_entry) {
            return Err(invalid());
        }
        state::check_path(&entry.path(), false)?;
        if entry.file_name() != "owner.lock" {
            paths.push(Payload {
                name: entry.file_name(),
                identity: state::LockIdentity::from_fd(state::private_reader(&entry.path())?)?,
            });
        }
    }
    paths.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(paths)
}
impl Retirement {
    pub(super) fn allocate(root: &Path, name: &str) -> Result<Self> {
        state::check_path(root, true)?;
        if !image_name(name) {
            return Err(invalid());
        }
        let original = root.join(name);
        if exists(&original)? || !matching_asides(&original)?.is_empty() {
            return Err(invalid());
        }
        fs::DirBuilder::new()
            .mode(crate::os::PRIVATE_DIRECTORY_MODE)
            .create(&original)
            .map_err(Error::state_io_from)?;
        Self::capture(&original)
    }
    pub(super) fn durable_owner(&self, owner: &state::Lock) -> Result<()> {
        self.check(&self.original)?;
        let marker = self.original.join("owner.lock");
        owner.sync_creation(&marker, &Boundary(&marker))?;
        Boundary(self.root()?).sync()?;
        self.check(&self.original)?;
        owner_at(&marker, owner)
    }

    /// Caller holds cache.lock. The local owner may supply its original held lock.
    pub(super) fn remove(&self, local: Option<&state::Lock>) -> Result<()> {
        self.check_parent()?;
        self.collisions()?;
        let directory = if exists(&self.original)? {
            &self.original
        } else {
            &self.aside
        };
        let acquired;
        let mut files = Vec::new();
        // The removal checks the directory by its live device: a named
        // retirement may predate a remount that renumbered it.
        let mut identity = self.tree;
        let owner = if exists(directory)? {
            identity = self.check(directory)?;
            if exists(&directory.join("owner.lock"))? {
                acquired = if local.is_none() {
                    Some(state::Lock::acquire_existing(
                        &directory.join("owner.lock"),
                    )?)
                } else {
                    None
                };
                let owner = local.or(acquired.as_ref()).ok_or_else(invalid)?;
                owner_at(&directory.join("owner.lock"), owner)?;
                files = payloads(directory)?;
                Some(owner)
            } else if directory == &self.aside {
                None
            } else {
                return Err(invalid());
            }
        } else {
            None
        };
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .with_barriers(&Boundary(&self.original))
            .remove_named_dir_using(&self.original, &self.aside, identity.directory(), |aside| {
                self.remove_flat(aside, owner, &files)
                    .map_err(io::Error::other)
            })
            .map_err(crate::publication::error)
    }
    fn remove_flat(
        &self,
        aside: &Path,
        owner: Option<&state::Lock>,
        files: &[Payload],
    ) -> Result<()> {
        self.check(aside)?;
        self.collisions()?;
        if let Some(owner) = owner {
            let marker = aside.join("owner.lock");
            owner_at(&marker, owner)?;
            if payloads(aside)? != files {
                return Err(invalid());
            }
            for payload in files {
                let path = aside.join(&payload.name);
                Boundary(&path)
                    .observe(Step::BeforeRemove)
                    .map_err(Error::state_io_from)?;
                self.check(aside)?;
                owner_at(&marker, owner)?;
                payload.check(aside)?;
                fs::remove_file(&path).map_err(Error::state_io_from)?;
                Boundary(&path)
                    .observe(Step::AfterRemove)
                    .map_err(Error::state_io_from)?;
            }
            // Never lose proof before payload absence is durable.
            Boundary(aside).sync()?;
            Boundary(&marker)
                .observe(Step::BeforeRemove)
                .map_err(Error::state_io_from)?;
            self.check(aside)?;
            owner_at(&marker, owner)?;
            if !payloads(aside)?.is_empty() {
                return Err(invalid());
            }
            fs::remove_file(&marker).map_err(Error::state_io_from)?;
            Boundary(&marker)
                .observe(Step::AfterRemove)
                .map_err(Error::state_io_from)?;
        }
        self.check(aside)?;
        // A markerless retry never authorizes file deletion, even for known names.
        fs::remove_dir(aside).map_err(Error::state_io_from)
    }
    pub(super) fn remove_empty_reservation(&self) -> Result<()> {
        self.check_parent()?;
        self.collisions()?;
        self.check(&self.original)?;
        fs::remove_dir(&self.original).map_err(Error::state_io_from)?;
        Boundary(self.root()?).sync()
    }
}

#[cfg(test)]
mod tests;
