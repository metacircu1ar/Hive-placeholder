use crate::Config;
use serde::Deserialize;
use std::io;
use tracing::{Metadata, level_filters::LevelFilter};
use tracing_log::{AsLog, AsTrace};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Policy {
    pub identity: Vec<String>,
    pub code: Vec<String>,
    pub number: Vec<String>,
    pub flag: Vec<String>,
    pub error: Vec<String>,
}
impl Policy {
    pub fn load() -> io::Result<Self> {
        serde_json::from_str(include_str!("fields.json")).map_err(io::Error::other)
    }
}
pub(crate) fn target_name(target: &str) -> bool {
    !target.is_empty()
        && target.len() <= 128
        && target
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_:-.".contains(&b))
}
fn workspace(target: &str) -> bool {
    [
        "hive",
        "hivectl",
        "hive_daemon",
        "comb",
        "comb_protocol",
        "hive_install",
        "hive_setup_support",
        "atomic_fs",
        "hive_log",
    ]
    .iter()
    .any(|root| {
        target == *root
            || target
                .strip_prefix(root)
                .is_some_and(|suffix| suffix.starts_with("::"))
    })
}
pub(crate) fn bridge(metadata: &Metadata<'_>) -> bool {
    let reference = log::Metadata::builder()
        .level(metadata.level().as_log())
        .target("log")
        .build();
    metadata.callsite() == reference.as_trace().callsite()
}
pub(crate) fn first_party(metadata: &Metadata<'_>) -> bool {
    !bridge(metadata)
        && metadata.module_path().is_some_and(workspace)
        && (workspace(metadata.target()) || metadata.target() == "hive.fatal")
}
pub(crate) fn fatal(metadata: &Metadata<'_>) -> bool {
    first_party(metadata) && metadata.target() == "hive.fatal"
}
impl Config {
    pub(crate) fn library_directive(&self, target: &str) -> Option<(&str, LevelFilter)> {
        self.library_levels
            .iter()
            .filter(|(prefix, _)| {
                target == prefix.as_str()
                    || target
                        .strip_prefix(prefix.as_str())
                        .is_some_and(|suffix| suffix.starts_with("::"))
            })
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(prefix, level)| (prefix.as_str(), *level))
    }
    pub(crate) fn library_level(&self, target: &str) -> LevelFilter {
        self.library_directive(target)
            .map_or(LevelFilter::WARN, |(_, level)| level)
    }
    pub(crate) fn library_max(&self) -> LevelFilter {
        self.library_levels
            .values()
            .copied()
            .fold(LevelFilter::WARN, LevelFilter::max)
    }
    pub(crate) fn accepts(&self, metadata: &Metadata<'_>) -> bool {
        if fatal(metadata) {
            return true;
        }
        let level = if first_party(metadata) {
            self.level
        } else if bridge(metadata) && metadata.name() == "log event" {
            self.library_max()
        } else {
            self.library_level(metadata.target())
        };
        *metadata.level() <= level
    }
}
