//! Structured failures shared by the CLI, socket and MCP boundaries.
mod causes;
mod codes;
mod diagnostics;
mod limits;
mod messages;
mod wire;
pub use causes::Cause;
pub use codes::ErrorCode;
use serde::Serialize;
use std::{borrow::Cow, sync::Arc};

#[derive(Debug, Clone)]
pub struct Error {
    pub code: ErrorCode,
    pub message: Cow<'static, str>,
    /// A durable file change may have succeeded even when indexing did not.
    pub file_committed: bool,
    pub request_id: Option<String>,
    pub import_progress: Option<Box<crate::import::ImportProgress>>,
    raw_code: Option<String>,
    input: Option<Box<hive_input::InputError>>,
    source: Option<Arc<dyn std::error::Error + Send + Sync>>,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|cause| cause as &dyn std::error::Error)
    }
}
pub type Result<T> = std::result::Result<T, Error>;
/// Promote errors only at an already-established file commit boundary.
pub trait CommitExt<T> {
    fn committed(self) -> Result<T>;
}

impl<T, E> CommitExt<T> for std::result::Result<T, E>
where
    Error: From<E>,
{
    fn committed(self) -> Result<T> {
        self.map_err(|error| Error::from(error).committed())
    }
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            code,
            message: message.into(),
            file_committed: false,
            request_id: None,
            import_progress: None,
            raw_code: None,
            source: None,
            input: None,
        }
    }
    #[must_use]
    pub fn caused_by(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.source = Some(Arc::new(source));
        self
    }
    fn caused_by_boxed(mut self, source: Box<dyn std::error::Error + Send + Sync>) -> Self {
        self.source = Some(Arc::from(source));
        self
    }
    #[must_use]
    pub fn committed(mut self) -> Self {
        self.file_committed = true;
        self
    }
    pub fn is_not_found(&self) -> bool {
        self.code == ErrorCode::NotFound
    }
    /// Validated code from a newer peer, when the local classification is unknown.
    pub fn raw_code(&self) -> Option<&str> {
        self.raw_code.as_deref()
    }
    /// Typed caller rejection, independent of overlapping native error codes.
    pub fn input(&self) -> Option<&hive_input::InputError> {
        self.input.as_deref()
    }
    pub fn is_space_change(&self) -> bool {
        self.code == ErrorCode::EmbeddingSpaceChanged
    }
    pub fn is_input_rejection(&self) -> bool {
        matches!(
            self.code,
            ErrorCode::EmbeddingInputTooLarge | ErrorCode::EmbeddingInputRejected
        )
    }
}
impl From<hive_input::InputError> for Error {
    fn from(input: hive_input::InputError) -> Self {
        let mut error = Self::new(input.code().into(), input.message().to_owned());
        error.input = Some(Box::new(input));
        error
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::io(e)
    }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::index(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::json(e)
    }
}
impl From<serde_yaml_ng::Error> for Error {
    fn from(e: serde_yaml_ng::Error) -> Self {
        Self::frontmatter(e)
    }
}

/// Convert public output through JSON's existing sorted-key representation.
pub fn value(input: impl Serialize) -> Result<serde_json::Value> {
    serde_json::to_value(input).map_err(|cause| Error::internal_serialization().caused_by(cause))
}

pub fn json_bytes(input: &(impl Serialize + ?Sized)) -> Result<Vec<u8>> {
    serde_json::to_vec(input).map_err(|cause| Error::internal_serialization().caused_by(cause))
}

pub fn json_text(input: &(impl Serialize + ?Sized)) -> Result<String> {
    serde_json::to_string(input).map_err(|cause| Error::internal_serialization().caused_by(cause))
}

pub fn yaml_text(input: &(impl Serialize + ?Sized)) -> Result<String> {
    serde_yaml_ng::to_string(input)
        .map_err(|cause| Error::internal_serialization().caused_by(cause))
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct OpeningFile(#[source] std::io::Error);
impl Error {
    pub fn opening_file(error: std::io::Error) -> Self {
        Self::io_open(error)
    }
    pub fn is_missing_file(&self) -> bool {
        self.source
            .as_ref()
            .and_then(|e| e.downcast_ref::<OpeningFile>())
            .is_some_and(|e| e.0.kind() == std::io::ErrorKind::NotFound)
    }
}

#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::unwrap_used)]
pub mod tests;
