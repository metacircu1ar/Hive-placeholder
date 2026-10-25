use super::{Context, Failure, Result, context::caller_io};
use comb::service::auth::CredentialOutput;
use hive_input::Code;
use std::{error::Error as _, path::Path};

impl Context<'_> {
    pub(in crate::cli) fn output_name(self, path: &Path) -> Result<()> {
        if path.file_name().is_some_and(|name| {
            hive_setup_support::names::state_preparation_name(name)
                || hive_setup_support::names::comb_admin_preparation_name(name)
        }) {
            return Err(self.invalid(
                Code::InvalidPath,
                "a filename outside the reserved .hive-tmp-state- and .comb-tmp-admin- \
                    preparation namespaces",
            ));
        }
        Ok(())
    }
    pub(in crate::cli) fn credential_output(self, path: &Path) -> Result<CredentialOutput> {
        self.output_name(path)?;
        let expected = "an unused UTF-8 filename in an existing owned directory without symlinks \
            or shared write access";
        if path.file_name().is_none() || path.to_str().is_none() || path.as_os_str().is_empty() {
            return Err(self.invalid(Code::InvalidPath, expected));
        }
        let absolute = std::path::absolute(path)
            .map_err(|cause| Failure::from(comb::Error::from(cause)).path(Some(path)))?;
        self.resolved_path(&absolute)?;
        CredentialOutput::prepare(&absolute).map_err(|error| {
            if comb::service::auth::has_cleanup_failure(&error) {
                return Failure::from(error).path(Some(path));
            }
            let mut cause = error.source();
            let mut caller = matches!(
                error.code,
                comb::ErrorCode::UnsafePath | comb::ErrorCode::CredentialExists
            );
            while let Some(current) = cause {
                if let Some(io) = current.downcast_ref::<std::io::Error>() {
                    caller |= caller_io(io) || io.kind() == std::io::ErrorKind::AlreadyExists;
                }
                cause = current.source();
            }
            if caller {
                self.invalid(Code::InvalidPath, expected)
            } else {
                Failure::from(error).path(Some(path))
            }
        })
    }
}
