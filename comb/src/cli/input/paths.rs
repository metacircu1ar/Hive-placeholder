//! Selected paths fail before knowledge mutation; stored corruption stays operational.
use super::{Context, Failure, Result, context::caller_io};
use comb::{Error, ErrorCode, service::auth::Connection};
use hive_input::Code;
use std::{fs, io, os::unix::fs::MetadataExt, path::Path};

impl Context<'_> {
    pub(in crate::cli) fn root(self, path: &Path) -> Result<()> {
        let selected = self.file(path);
        let meta = fs::symlink_metadata(path).map_err(|cause| selected.root_io(cause, path))?;
        if meta.file_type().is_symlink() {
            return Err(Failure::from(Error::unsafe_directory()).path(Some(path)));
        }
        if !meta.is_dir() {
            return Err(selected.invalid(Code::InvalidPath, "an existing Comb root directory"));
        }
        if meta.uid() != rustix::process::geteuid().as_raw()
            || !hive_setup_support::directories::owned(
                meta.uid(),
                meta.mode(),
                rustix::process::geteuid().as_raw(),
            )
        {
            return Err(Failure::from(Error::unsafe_directory()).path(Some(path)));
        }
        match fs::symlink_metadata(path.join(".comb")) {
            Err(cause) if cause.kind() == io::ErrorKind::NotFound => Err(selected.invalid(
                Code::InvalidPath,
                "an initialized Comb root; initialize a new root with comb init DIR",
            )),
            Err(cause) => Err(selected.root_io(cause, path)),
            Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
                Err(Failure::from(Error::unsafe_directory()).path(Some(path)))
            }
            Ok(meta)
                if meta.uid() != rustix::process::geteuid().as_raw()
                    || meta.mode() & 0o077 != 0 =>
            {
                Err(Failure::from(Error::unsafe_admin_directory()).path(Some(path)))
            }
            Ok(_) => Ok(()),
        }
    }

    fn root_io(self, cause: io::Error, path: &Path) -> Failure {
        if caller_io(&cause) {
            self.invalid(
                Code::InvalidPath,
                "an existing accessible Comb root; initialize a new root with comb init DIR",
            )
        } else {
            Failure::from(Error::from(cause)).path(Some(path))
        }
    }

    pub(in crate::cli) fn credential(self, path: &Path) -> Result<Connection> {
        let selected = self.file(path);
        Connection::read(path).map_err(|error| {
            let caller = matches!(
                error.code,
                ErrorCode::InvalidJson
                    | ErrorCode::InvalidIdentity
                    | ErrorCode::InvalidCredential
                    | ErrorCode::InvalidSocket
                    | ErrorCode::UnsafeCredential
                    | ErrorCode::UnsafePath
                    | ErrorCode::SizeLimit
            ) || super::embedding::error_io(&error).is_some_and(caller_io);
            if caller {
                selected.invalid(
                    Code::InvalidFile,
                    "an owned private regular credential file (0600), at most 16384 bytes, \
                        containing a valid Comb connection object and absolute socket path",
                )
            } else {
                Failure::from(error).path(Some(path))
            }
        })
    }
}

#[cfg(test)]
mod tests;
