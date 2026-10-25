//! Explicit config files are prepared before knowledge or service access.
use super::{Context, Failure, Result, reader};
use comb::{
    Error,
    embedding::{CONFIG_BYTES, Config},
};
use hive_input::Code;
use std::path::Path;

impl Context<'_> {
    pub(in crate::cli) fn socket_output(
        self,
        path: &Path,
        checked: comb::Result<()>,
    ) -> Result<()> {
        checked.map_err(|error| {
            if matches!(
                error.code,
                comb::ErrorCode::UnsafePath
                    | comb::ErrorCode::InvalidSocket
                    | comb::ErrorCode::SocketPathTooLong
                    | comb::ErrorCode::InvalidEmbeddingConfig
            ) || error_io(&error).is_some_and(super::context::caller_io)
            {
                self.file(path)
                    .invalid(
                        Code::InvalidPath,
                        "a socket path within 103 absolute bytes, in an owned mode-0700 \
                            directory beneath safe ancestors; setup can create missing \
                            directories",
                    )
                    .caused_by(error)
            } else {
                Failure::from(error).path(Some(path))
            }
        })
    }
    pub(in crate::cli) fn embedding(self, path: &Path) -> Result<Config> {
        let config = self.read_embedding(path)?;
        config.validate_input().map_err(|error| {
            let Some(input) = error.input() else {
                return Failure::from(Error::internal_serialization().caused_by(error));
            };
            self.file(path)
                .invalid(input.code(), input.expected())
                .caused_by(error)
        })?;
        Ok(config)
    }
    // Setup enforces its HTTP backend requirement before validating scalar fields.
    pub(in crate::cli) fn read_embedding(self, path: &Path) -> Result<Config> {
        let selected = self.file(path);
        let source = reader::source(path, CONFIG_BYTES, reader::PIPE_DEADLINE)
            .map_err(|cause| selected.io(cause))?;
        hive_input::validate::byte_count(source.bytes.len(), CONFIG_BYTES)
            .map_err(|rule| selected.invalid(rule.code(), rule.expected()))?;
        let absolute = std::path::absolute(path)
            .map_err(|cause| Failure::from(Error::from(cause)).path(Some(path)))?;
        Config::parse(
            &source.bytes,
            source.regular.then(|| absolute.parent()).flatten(),
        )
        .map_err(|error| {
            selected
                .invalid(Code::InvalidFile, &error.message)
                .caused_by(error)
        })
    }
    pub(in crate::cli) fn socket(self, root: &Path, path: &Path) -> Result<std::path::PathBuf> {
        let selected = self.file(path);
        comb::service::socket_path(root, Some(path)).map_err(|error| {
            if matches!(
                error.code,
                comb::ErrorCode::InvalidSocket | comb::ErrorCode::SocketPathTooLong
            ) {
                selected.invalid(
                    Code::InvalidPath,
                    "a socket filename in an accessible directory, within 103 bytes when absolute",
                )
            } else if let Some(io) = error_io(&error) {
                if super::context::caller_io(io) {
                    selected.invalid(
                        Code::InvalidPath,
                        "a socket filename in an existing accessible directory",
                    )
                } else {
                    Failure::from(error).path(Some(path))
                }
            } else {
                Failure::from(error).path(Some(path))
            }
        })
    }
}
pub(super) fn error_io(error: &Error) -> Option<&std::io::Error> {
    use std::error::Error as _;
    let mut source = error.source();
    while let Some(cause) = source {
        if let Some(io) = cause.downcast_ref() {
            return Some(io);
        }
        source = cause.source();
    }
    None
}

#[cfg(test)]
mod tests;
