//! Operator-visible level and rotation limits; queue/privacy policy stays fixed.
use super::{Error, Result};
use serde::Deserialize;
use tracing::level_filters::LevelFilter;

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}
impl LogLevel {
    fn filter(self) -> LevelFilter {
        match self {
            Self::Off => LevelFilter::OFF,
            Self::Error => LevelFilter::ERROR,
            Self::Warn => LevelFilter::WARN,
            Self::Info => LevelFilter::INFO,
            Self::Debug => LevelFilter::DEBUG,
            Self::Trace => LevelFilter::TRACE,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Logging {
    pub level: LogLevel,
    pub file_bytes: u64,
    pub files: usize,
}
impl Default for Logging {
    fn default() -> Self {
        let defaults = hive_log::Config::default();
        Self {
            level: LogLevel::Info,
            file_bytes: defaults.file_bytes,
            files: defaults.files,
        }
    }
}
impl Logging {
    pub(crate) fn backend(&self) -> Result<hive_log::Config> {
        let config = hive_log::Config {
            level: self.level.filter(),
            file_bytes: self.file_bytes,
            files: self.files,
            ..Default::default()
        };
        config.validate().map_err(|error| {
            Error::config_invalid(
                "Invalid logging retention: use 2..64 files and 12288..1073741824 bytes per file.",
            )
            .with_source(error)
        })?;
        Ok(config)
    }
}
