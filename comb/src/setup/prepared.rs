//! Retain explicit caller configuration across state reinspection and service pause.
use super::{Error, Options, Outcome, Path, Result, input};
use crate::embedding::{Config, HttpConfig};
use hive_input::Code;

#[derive(Clone)]
pub struct Prepared {
    pub(super) options: Options,
    pub(super) http: Option<HttpConfig>,
}
impl Options {
    pub fn prepare(self) -> Result<Prepared> {
        self.validate_input()?;
        let config = self
            .embedding_config
            .as_deref()
            .map(read_config)
            .transpose()?;
        Prepared::from_config(self, config)
    }
}
impl Prepared {
    /// Use a configuration already read by a CLI input adapter, including pipe input.
    pub fn from_config(options: Options, config: Option<Config>) -> Result<Self> {
        options.validate_input()?;
        if options.embedding_config.is_some() != config.is_some() {
            return Err(Error::internal_serialization());
        }
        let http = config.map(http).transpose()?;
        Ok(Self { options, http })
    }
    pub fn options(&self) -> &Options {
        &self.options
    }
    pub fn run(self, root: &Path, socket: Option<&Path>) -> Result<Outcome> {
        crate::services::setup(root, socket, self)
    }
}
fn http(config: Config) -> Result<HttpConfig> {
    let Config::Http(config) = config;
    Config::Http(config.clone())
        .validate_input()
        .map_err(|cause| {
            let expected = cause
                .input()
                .map_or("valid HTTP embedding configuration", |input| {
                    input.expected()
                });
            input::error(Code::InvalidValue, "embedding_config", expected).caused_by(cause)
        })?;
    Ok(config)
}
fn read_config(path: &Path) -> Result<Config> {
    Config::read(path).map_err(|cause| {
        if cause.code == crate::ErrorCode::InvalidEmbeddingConfig || caller_io(&cause) {
            input::error(
                Code::InvalidFile,
                "embedding_config",
                "a readable regular file containing valid HTTP embedding configuration",
            )
            .caused_by(cause)
        } else {
            cause
        }
    })
}
fn caller_io(error: &Error) -> bool {
    use std::error::Error as _;
    let Some(cause) = error
        .source()
        .and_then(|cause| cause.downcast_ref::<std::io::Error>())
    else {
        return false;
    };
    matches!(
        cause.kind(),
        std::io::ErrorKind::NotFound
            | std::io::ErrorKind::PermissionDenied
            | std::io::ErrorKind::InvalidInput
            | std::io::ErrorKind::NotADirectory
            | std::io::ErrorKind::IsADirectory
    ) || matches!(
        cause.raw_os_error(),
        Some(libc::ELOOP | libc::ENAMETOOLONG | libc::ENXIO | libc::ENODEV | libc::EOPNOTSUPP)
    )
}
