use super::{Failure, Result, reader};
use comb::{Error, Page, page::PAGE_BYTES};
use hive_input::{Code, InputError};
use std::{
    io::{self, Read},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy)]
pub(in crate::cli) struct Context<'a> {
    command: &'static str,
    field: &'static str,
    path: Option<&'a Path>,
}
impl<'a> Context<'a> {
    pub(in crate::cli) fn new(command: &'static str, field: &'static str) -> Self {
        Self {
            command,
            field,
            path: None,
        }
    }
    pub(in crate::cli) fn file(self, path: &'a Path) -> Self {
        self.optional_file(Some(path))
    }
    pub(in crate::cli) fn optional_file(self, path: Option<&'a Path>) -> Self {
        Self { path, ..self }
    }
    pub(super) fn invalid(self, code: Code, expected: &str) -> Failure {
        let error = crate::cli::catalog::examples(self.command)
            .ok_or_else(Error::internal_serialization)
            .and_then(|examples| {
                InputError::new(
                    code,
                    "Invalid command input.",
                    self.field,
                    expected,
                    examples.as_slice().to_vec(),
                )
                .map_err(|cause| Error::internal_serialization().caused_by(cause))
            });
        match error {
            Ok(error) => Failure::input(error).path(self.path),
            Err(error) => Failure::from(error),
        }
    }
    pub(super) fn io(self, cause: io::Error) -> Failure {
        if caller_io(&cause) {
            let (code, expected) = if self.field == "DIR" {
                (
                    Code::InvalidPath,
                    "an existing accessible directory with a UTF-8 absolute path",
                )
            } else {
                (
                    Code::InvalidFile,
                    "a readable regular file or single-consumer pipe reaching EOF within 30 \
                        seconds",
                )
            };
            self.invalid(code, expected).caused_by(Error::from(cause))
        } else {
            Failure::from(Error::from(cause)).path(self.path)
        }
    }
    fn text(self) -> Result<String> {
        let bytes = if let Some(path) = self.path {
            reader::read(path, PAGE_BYTES, reader::PIPE_DEADLINE).map_err(|cause| self.io(cause))?
        } else {
            let mut bytes = Vec::new();
            std::io::stdin()
                .lock()
                .take(PAGE_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|cause| self.io(cause))?;
            bytes
        };
        self.decode(bytes)
    }
    pub(super) fn decode(self, bytes: Vec<u8>) -> Result<String> {
        hive_input::validate::byte_count(bytes.len(), PAGE_BYTES)
            .map_err(|rule| self.invalid(rule.code(), rule.expected()))?;
        String::from_utf8(bytes).map_err(|_| self.invalid(Code::InvalidFile, "UTF-8 text"))
    }
    pub(in crate::cli) fn markdown(self) -> Result<String> {
        let text = self.text()?;
        Page::parse(&text).map_err(|_| {
            self.invalid(
                Code::InvalidValue,
                "valid Comb Markdown with title frontmatter and valid page metadata/timeline",
            )
        })?;
        Ok(text)
    }
    pub(in crate::cli) fn summary(self, nonblank: bool) -> Result<String> {
        let text = self.text()?;
        if text.contains('\0') || (nonblank && text.trim().is_empty()) {
            let expected = if nonblank {
                "nonblank UTF-8 summary text without NUL bytes"
            } else {
                "UTF-8 summary text without NUL bytes; an empty summary clears it"
            };
            return Err(self.invalid(Code::InvalidValue, expected));
        }
        Ok(text)
    }
    pub(in crate::cli) fn directory(self) -> Result<PathBuf> {
        let path = self.path.ok_or_else(Error::internal_serialization)?;
        if path.to_str().is_none() || path.as_os_str().is_empty() {
            return Err(self.invalid(Code::InvalidPath, "an existing directory with a UTF-8 path"));
        }
        let resolved = path.canonicalize().map_err(|cause| self.io(cause))?;
        self.resolved_path(&resolved)?;
        if !resolved
            .metadata()
            .map_err(|cause| self.io(cause))?
            .is_dir()
        {
            return Err(self.invalid(Code::InvalidPath, "an existing directory"));
        }
        // Search permission permits metadata without permission to enumerate.
        // Prove the import source is readable before opening knowledge state.
        std::fs::read_dir(&resolved).map_err(|cause| self.io(cause))?;
        Ok(resolved)
    }
    pub(super) fn resolved_path(self, path: &Path) -> Result<()> {
        hive_input::validate::absolute_path(path)
            .map_err(|rule| self.invalid(rule.code(), rule.expected()))
    }
}

pub(super) fn caller_io(cause: &io::Error) -> bool {
    matches!(
        cause.kind(),
        io::ErrorKind::NotFound
            | io::ErrorKind::PermissionDenied
            | io::ErrorKind::InvalidInput
            | io::ErrorKind::NotADirectory
            | io::ErrorKind::IsADirectory
            | io::ErrorKind::TimedOut
    ) || matches!(
        cause.raw_os_error(),
        Some(libc::ELOOP | libc::ENAMETOOLONG | libc::ENXIO | libc::ENODEV | libc::EOPNOTSUPP)
    )
}
