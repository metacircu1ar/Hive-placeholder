//! Reject unknown namespace entries before interpreting missing ownership evidence.
use crate::{
    error::{Error, Result},
    state::{check_path, staging_file},
};
use std::{fs, path::Path};

#[derive(Clone, Copy)]
pub(super) enum Namespace {
    Profile,
    Audit,
    Probe,
}

pub(super) fn check(dir: &Path, namespace: Namespace) -> Result<()> {
    check_path(dir, true)?;
    for entry in fs::read_dir(dir).map_err(Error::state_io_from)? {
        let entry = entry.map_err(Error::state_io_from)?;
        let path = entry.path();
        if staging_file(&path)? {
            continue;
        }
        let directory = match (entry.file_name().to_str(), namespace) {
            (Some("provider" | "work" | "executions" | "execution-archive"), _)
            | (
                Some("writers" | "recovery-receipts" | "credential-resolutions"),
                Namespace::Profile | Namespace::Audit,
            )
            | (Some("provider.pruning" | "work.pruning"), Namespace::Audit) => true,
            (Some("auth.lock" | "evidence.lock" | "identity.json"), _)
            | (Some("prune-receipt.json"), Namespace::Profile | Namespace::Audit)
            | (Some("prune-intent.json" | "native-removal.json"), Namespace::Audit) => false,
            _ => {
                return Err(Error::unsupported_schema(concat!(
                    "Unsupported credential namespace layout. Use a new state directory; ",
                    "retained ownership evidence was not modified.",
                )));
            }
        };
        check_path(&path, directory)?;
    }
    Ok(())
}

pub(in crate::state) fn profile(dir: &Path) -> Result<()> {
    check(dir, Namespace::Profile)
}

/// Audit inspection does not grant credential admission or deletion authority.
pub(in crate::state) fn audit_profile(dir: &Path) -> Result<()> {
    check(dir, Namespace::Audit)
}
