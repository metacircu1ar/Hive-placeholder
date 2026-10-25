//! The replacement root becomes visible only with its complete namespace fence.
use super::{Error, Provider, Result, State, corrupt, os};
use std::{
    fs::OpenOptions,
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

pub(super) fn create(
    old: &State,
    destination: &Path,
    provider: Provider,
    barriers: &impl atomic_fs::Barriers,
) -> Result<State> {
    let destination = old.replacement_destination(destination)?;
    let _admission = State::new(destination.clone())?.admission()?;
    let retired = old.replacement_metadata(provider)?;
    let mut bytes = serde_json::to_vec(&retired).map_err(|cause| corrupt().with_source(cause))?;
    bytes.push(b'\n');
    atomic_fs::Publisher::new(hive_setup_support::names::PUBLICATION_OPTIONS)
        .with_barriers(barriers)
        .staged_dir(&destination, |stage| {
            crate::state::format::prepare(stage)?;
            // Private, unpublished R3 preparation. The outer helper flushes this file
            // and directory before its single no-replace directory commit.
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(os::PRIVATE_FILE_MODE)
                .custom_flags(libc::O_NOFOLLOW)
                .open(stage.join("retired-controller.json"))?
                .write_all(&bytes)
        })
        // Retain publication and cleanup evidence, including a committed destination.
        // Never remove that destination or retry publication after an uncertain sync.
        .map_err(|cause| Error::state_io_from(io::Error::other(cause)))?;
    State::new(destination)
}

#[cfg(test)]
mod tests;
