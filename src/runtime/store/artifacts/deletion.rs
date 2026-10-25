//! Invalidate derived references durably before deleting committed output.
use super::{Error, Result, missing_file, state};
use atomic_fs::{Barriers, Step};
use std::{fs, io, path::Path};

struct Cleanup;
impl Barriers for Cleanup {
    #[cfg(test)]
    fn observe(&self, step: Step) -> io::Result<()> {
        tests::step(step)
    }
}

pub(in crate::runtime) fn unlink_artifacts(dir: &Path, ids: &[String]) -> Result<()> {
    state::check_path(dir, true)?;
    // SQL uses exact text equality, but filenames can be case-insensitive. Never
    // let an alternate spelling bypass the retained-output conflict check.
    if ids.iter().any(|id| {
        !matches!(uuid::Uuid::parse_str(id), Ok(parsed) if parsed.hyphenated().to_string() == *id)
    }) {
        return Err(Error::database_error("Invalid artifact deletion identity."));
    }
    let runtime = dir
        .parent()
        .ok_or(Error::invalid_path("Missing runtime directory."))?;
    remove_checked(&runtime.join("output-manifest.jsonl"), &Cleanup)?;
    // Repeat even if already absent: an earlier attempt may have removed the
    // manifest but failed its parent sync. No artifact can be removed before it.
    sync_directory(runtime, &Cleanup)?;
    for id in ids {
        remove_checked(&dir.join(format!("{id}.json")), &Cleanup)?;
    }
    // The caller acknowledges the durable outbox only after this final barrier.
    sync_directory(dir, &Cleanup)
}

fn remove_checked(path: &Path, barriers: &impl Barriers) -> Result<()> {
    match state::check_path(path, false) {
        Err(error) if missing_file(&error) => return Ok(()),
        checked => checked?,
    }
    barriers
        .observe(Step::BeforeRemove)
        .map_err(Error::state_io_from)?;
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        result => result.map_err(Error::state_io_from)?,
    }
    barriers
        .observe(Step::AfterRemove)
        .map_err(Error::state_io_from)
}

fn sync_directory(path: &Path, barriers: &impl Barriers) -> Result<()> {
    let sync = || -> io::Result<()> {
        barriers.observe(Step::BeforeDirectorySync)?;
        barriers.sync_directory(&fs::File::open(path)?)?;
        barriers.observe(Step::AfterDirectorySync)
    };
    sync().map_err(Error::state_io_from)
}

#[cfg(test)]
mod tests;
