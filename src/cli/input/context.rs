use super::{Failure, Result, reader};
use crate::error::Error;
use hive_input::{Code, InputError};
use std::{io, path::Path};

#[derive(Clone, Copy)]
pub(in crate::cli) struct Context<'a> {
    command: &'static str,
    flag: &'static str,
    path: Option<&'a Path>,
}
impl<'a> Context<'a> {
    pub(in crate::cli) fn new(command: &'static str, flag: &'static str) -> Self {
        Self {
            command,
            flag,
            path: None,
        }
    }
    pub(in crate::cli) fn file(self, path: &'a Path) -> Self {
        Self {
            path: Some(path),
            ..self
        }
    }

    pub(in crate::cli) fn invalid(
        self,
        code: Code,
        message: &'static str,
        expected: &str,
    ) -> Failure {
        let error = super::super::catalog::examples(self.command)
            .ok_or_else(|| Error::internal_error("Missing CLI input examples."))
            .and_then(|examples| {
                InputError::new(
                    code,
                    message,
                    self.flag,
                    expected,
                    examples.as_slice().to_vec(),
                )
                .map_err(|cause| {
                    Error::internal_error("Invalid CLI input metadata.").with_source(cause)
                })
            })
            .map_or_else(|error| error, Error::from);
        let exit_code = if matches!(error, Error::Input { .. }) {
            2
        } else {
            1
        };
        Failure {
            error,
            path: self.path.map(Path::to_path_buf),
            exit_code,
        }
    }

    pub(super) fn io(self, cause: io::Error) -> Failure {
        if cause.kind() == io::ErrorKind::TimedOut {
            return self
                .invalid(
                    Code::InvalidFile,
                    "Input pipe did not finish before the read deadline.",
                    "a single-consumer pipe that reaches EOF within 30 seconds",
                )
                .with_source(cause);
        }
        self.native(
            cause,
            Code::InvalidFile,
            "Cannot read the selected input file.",
            "a readable regular file or single-consumer pipe",
        )
    }

    fn native(
        self,
        cause: io::Error,
        code: Code,
        message: &'static str,
        expected: &str,
    ) -> Failure {
        let failure = if crate::os::input_path_error(&cause) {
            self.invalid(code, message, expected)
        } else {
            Failure {
                error: Error::state_io("Cannot read command input."),
                path: self.path.map(Path::to_path_buf),
                exit_code: 1,
            }
        };
        failure.with_source(cause)
    }

    pub(super) fn directory_io(self, cause: io::Error) -> Failure {
        self.native(
            cause,
            Code::InvalidPath,
            "Cannot access the selected working directory.",
            "an existing accessible directory",
        )
    }

    pub(in crate::cli) fn directory(self) -> Result<std::path::PathBuf> {
        let path = self
            .path
            .ok_or_else(|| Error::internal_error("Missing CLI directory path."))?;
        if path.as_os_str().is_empty() {
            return Err(self.invalid(
                Code::InvalidPath,
                "Working directory must not be empty.",
                "an existing directory path",
            ));
        }
        let absolute = std::path::absolute(path).map_err(|cause| self.directory_io(cause))?;
        self.resolved_directory(absolute)
    }

    pub(super) fn resolved_directory(
        self,
        absolute: std::path::PathBuf,
    ) -> Result<std::path::PathBuf> {
        // Check the complete resolved path: even ASCII relative input can acquire
        // a non-UTF8 prefix from the invoking directory. JSON paths require UTF-8.
        if absolute.to_str().is_none() {
            return Err(self.invalid(
                Code::InvalidPath,
                "Working directory must be valid UTF-8.",
                "a directory whose absolute path is UTF-8",
            ));
        }
        hive_input::validate::absolute_path(&absolute).map_err(|rule| {
            self.invalid(
                rule.code(),
                "Invalid working directory path.",
                rule.expected(),
            )
            .with_source(rule)
        })?;
        let metadata = std::fs::metadata(&absolute).map_err(|cause| self.directory_io(cause))?;
        if !metadata.is_dir() {
            return Err(self.invalid(
                Code::InvalidPath,
                "Working directory is not a directory.",
                "an existing directory",
            ));
        }
        Ok(absolute)
    }

    pub(in crate::cli) fn bytes(self, cap: usize) -> Result<Vec<u8>> {
        let path = self
            .path
            .ok_or_else(|| Error::internal_error("Missing CLI input path."))?;
        let bytes =
            reader::read(path, cap, reader::PIPE_DEADLINE).map_err(|cause| self.io(cause))?;
        self.size(bytes.len(), cap)?;
        Ok(bytes)
    }
    fn size(self, length: usize, cap: usize) -> Result<()> {
        hive_input::validate::byte_count(length, cap).map_err(|rule| {
            self.invalid(
                rule.code(),
                "Input exceeds the byte limit.",
                rule.expected(),
            )
            .with_source(rule)
        })
    }
    pub(in crate::cli) fn text(self, cap: usize, allow_empty: bool) -> Result<String> {
        let bytes = self.bytes(cap)?;
        let text = String::from_utf8(bytes).map_err(|cause| {
            self.invalid(
                Code::InvalidFile,
                "Input file is not valid UTF-8.",
                "UTF-8 text",
            )
            .with_source(cause)
        })?;
        self.checked_text(text, cap, allow_empty)
    }
    pub(in crate::cli) fn checked_text(
        self,
        text: String,
        cap: usize,
        allow_empty: bool,
    ) -> Result<String> {
        self.size(text.len(), cap)?;
        if !allow_empty && text.is_empty() {
            return Err(self.invalid(
                Code::InvalidValue,
                "Input must not be empty.",
                "nonempty UTF-8 text",
            ));
        }
        Ok(text)
    }
    pub(in crate::cli) fn json(self, bytes: &[u8], cap: usize) -> Result<serde_json::Value> {
        self.size(bytes.len(), cap)?;
        serde_json::from_slice(bytes).map_err(|cause| {
            self.invalid(
                Code::InvalidJson,
                "Input is not valid JSON.",
                "a JSON value",
            )
            .with_source(cause)
        })
    }
}
