//! Immutable removal records and repeatable durability barriers.
use crate::{
    error::{Error, Result},
    publication, state,
};
use atomic_fs::{Barriers, PreparedCreate, Step};
use serde::Serialize;
use std::{fs::File, path::Path};

pub(in crate::state) struct Boundary<'a>(pub &'a Path);
impl Barriers for Boundary<'_> {
    #[cfg(test)]
    fn observe(&self, step: Step) -> std::io::Result<()> {
        state::sync_tests::step(self.0, step)
    }
}
impl Boundary<'_> {
    pub(in crate::state) fn sync_parent(&self) -> Result<()> {
        let parent = self.0.parent().ok_or_else(state::unsafe_state)?;
        state::check_path(parent, true)?;
        self.sync_at(parent)
    }
    fn sync_at(&self, directory: &Path) -> Result<()> {
        self.observe(Step::BeforeDirectorySync)
            .and_then(|()| Barriers::sync_directory(self, &File::open(directory)?))
            .and_then(|()| self.observe(Step::AfterDirectorySync))
            .map_err(Error::state_io_from)
    }
    pub(in crate::state) fn sync_directory(&self) -> Result<()> {
        state::check_path(self.0, true)?;
        self.sync_at(self.0)
    }
}

pub(in crate::state) fn publish(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut bytes =
        serde_json::to_vec(value).map_err(|cause| state::corrupt().with_source(cause))?;
    bytes.push(b'\n');
    PreparedCreate::new(
        path,
        (hive_setup_support::names::PUBLICATION_OPTIONS)
            .with_prefix(hive_setup_support::names::STATE_PREFIX),
    )
    .and_then(|prepared| prepared.publish_with(&bytes, &Boundary(path)))
    .map_err(publication::error)
}

/// A published record may have survived an unsuccessful parent sync. Finish its
/// barriers before using it as authority for the next destructive phase.
pub(in crate::state) fn sync_record(path: &Path) -> Result<()> {
    let file = state::private_reader(path)?;
    let boundary = Boundary(path);
    boundary
        .observe(Step::BeforeFileSync)
        .and_then(|()| boundary.sync_file(&file))
        .and_then(|()| boundary.observe(Step::AfterFileSync))
        .map_err(Error::state_io_from)?;
    boundary.sync_parent()
}
