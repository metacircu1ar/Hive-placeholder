//! Strict offline CLI options are checked before recovery under the writer guard.
use super::Space;
use crate::{Comb, Error, Result, index::Index};
use hive_input::{Code, InputError};
use std::path::Path;

impl Comb {
    /// Open for an explicit offline embedding request. Unlike idempotent native
    /// activation, this rejects an unnecessary replacement option.
    pub fn open_for_embedding(root: &Path, space: &Space, replace: bool) -> Result<Self> {
        space.validate()?;
        let fingerprint = space.fingerprint()?;
        Self::open_checked(root, &|disk| {
            disk.reject_pending_rebuild()?;
            let active = Index::existing_space(&disk.index_path())?;
            match (active, replace) {
                (None, true) => Err(input(
                    Code::ConflictingArguments,
                    "omit --replace on first activation",
                )),
                (Some((lease, _)), true) if lease.fingerprint == fingerprint => Err(input(
                    Code::ConflictingArguments,
                    "omit --replace to backfill or retry the already active space",
                )),
                (Some((lease, _)), false) if lease.fingerprint != fingerprint => Err(input(
                    Code::MissingArgument,
                    "--replace to change the active embedding space",
                )
                .caused_by(Error::embedding_replace_required())),
                _ => Ok(()),
            }
        })
    }
}
fn input(code: Code, expected: &str) -> Error {
    match InputError::new(
        code,
        "Invalid offline embedding replacement option.",
        "/options/replace",
        expected,
        vec!["comb embed --config embeddings.yaml".into()],
    ) {
        Ok(error) => error.into(),
        Err(cause) => Error::internal_serialization().caused_by(cause),
    }
}

#[cfg(test)]
mod tests;
