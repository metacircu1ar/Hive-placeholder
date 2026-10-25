//! Skill publication retains typed conflicts and accepted/native failure evidence.
use super::{Result, files, invalid, read_owned, safe};
use crate::error::Error;
use atomic_fs::{Barriers, PreparedCreate, Step};
use hive_setup_support::checkpoints::Checkpoint;
use std::{io as stdio, path::Path};

pub(super) fn io(cause: stdio::Error) -> Error {
    let public = cause
        .get_ref()
        .and_then(|e| e.downcast_ref::<Error>())
        .cloned();
    public
        .unwrap_or_else(|| {
            if cause
                .get_ref()
                .and_then(|e| e.downcast_ref::<atomic_fs::Error>())
                .is_some_and(atomic_fs::Error::published)
            {
                Error::state_io(
                    "Skill ownership change is accepted, but its durability is unconfirmed; \
                    rerun setup after correcting the cause.",
                )
            } else if cause.kind() == stdio::ErrorKind::InvalidInput {
                invalid("Invalid skill publication state.")
            } else {
                Error::state_io(
                    "Cannot safely publish skill copies; correct the cause and rerun setup.",
                )
            }
        })
        .with_source(cause)
}

pub(super) fn error(cause: atomic_fs::Error) -> Error {
    let public = cause
        .io_error()
        .get_ref()
        .and_then(|e| e.downcast_ref::<Error>())
        .cloned();
    public
        .unwrap_or_else(|| {
            if cause.published() {
                Error::state_io(
                    "Skill copy change is accepted, but its durability is unconfirmed; rerun \
                setup after correcting the cause.",
                )
            } else {
                Error::state_io(
                    "Skill copy publication failed before replacement; rerun setup after \
                correcting the cause.",
                )
            }
        })
        .with_source(cause)
}

#[cfg_attr(
    not(test),
    allow(
        clippy::unnecessary_wraps,
        reason = "Test fault injection must preserve fallible intent boundaries at the real \
            call sites"
    )
)]
pub(super) fn checkpoint(path: &Path, step: impl Into<Checkpoint>) -> stdio::Result<()> {
    #[cfg(not(test))]
    let _ = (path, step);
    #[cfg(test)]
    tests::checkpoint(path, step.into())?;
    Ok(())
}

pub(super) struct Publication<'a> {
    pub path: &'a Path,
    pub current: Option<&'a [u8]>,
}
impl Publication<'_> {
    pub fn check(&self) -> stdio::Result<()> {
        safe(self.path).map_err(stdio::Error::other)?;
        if read_owned(self.path)
            .map_err(stdio::Error::other)?
            .as_deref()
            != self.current
        {
            return Err(stdio::Error::other(invalid(
                "Skill changed during publication.",
            )));
        }
        Ok(())
    }
}
impl Barriers for Publication<'_> {
    fn observe(&self, step: Step) -> stdio::Result<()> {
        checkpoint(self.path, step)?;
        if matches!(step, Step::BeforePublish | Step::BeforeRemove) {
            self.check()?;
        }
        Ok(())
    }
}

pub(super) fn update(path: &Path, current: Option<&[u8]>, wanted: Option<&[u8]>) -> Result<()> {
    let barriers = Publication { path, current };
    let Some(bytes) = wanted else {
        return remove(path, &barriers);
    };
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Skill needs a parent."))?;
    files::directory(parent).map_err(io)?;
    if current.is_some() {
        atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
            .with_barriers(&barriers)
            .write(path, bytes)
    } else {
        PreparedCreate::new(path, hive_setup_support::names::PUBLICATION_OPTIONS)
            .and_then(|prepared| prepared.publish_with(bytes, &barriers))
    }
    .map_err(error)
}

fn remove(path: &Path, barriers: &Publication<'_>) -> Result<()> {
    let directory = super::removal::unlink(path, barriers)?;
    let result = barriers
        .observe(Step::AfterRemove)
        .and_then(|()| super::recovery::synchronize(path, None, Some(directory)));
    result.map_err(|source| error(atomic_fs::Error::new(source, true)))
}

#[cfg(test)]
pub(super) mod tests;
