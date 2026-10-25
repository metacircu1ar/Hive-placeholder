#![allow(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact the reviewed maintainer write inventory inventory"
)]
//! Shared local knowledge: authoritative Markdown and a rebuildable retrieval index.
#[doc(hidden)]
pub mod bounded;
mod chunk;
pub mod compatibility;
mod convert;
mod disk;
pub mod embedding;
pub mod error;
#[cfg(test)]
mod golden;
mod import;
mod index;
mod locks;
#[doc(hidden)]
pub mod maintenance;
pub mod page;
mod random;
mod read;
pub mod search;
pub mod service;
pub mod services;
pub mod setup;
pub mod store;
#[cfg(test)]
mod test_support;
pub mod values;

pub use disk::UpgradeReport;
pub use error::{Error, ErrorCode, Result};
pub use index::Index;
pub use page::{Actor, Page};
pub use store::Comb;

/// Product identity for setup diagnostics; this is not a storage format version.
#[derive(serde::Serialize)]
pub struct BuildInfo {
    pub product: &'static str,
    pub product_version: &'static str,
    pub commit: Option<&'static str>,
    pub compiled_features: &'static [&'static str],
    pub formats: &'static [hive_input::compatibility::Entry],
    pub shared_formats: SharedFormats,
    pub mcp_protocols: &'static [&'static str],
}
#[derive(serde::Serialize)]
pub struct SharedFormats {
    pub setup: &'static [hive_input::compatibility::Entry],
    pub comb_protocol: &'static [hive_input::compatibility::Entry],
}
pub fn build_info() -> BuildInfo {
    BuildInfo {
        product: "Comb",
        product_version: env!("CARGO_PKG_VERSION"),
        commit: hive_setup_support::identity::COMMIT,
        formats: compatibility::FORMATS,
        mcp_protocols: comb_protocol::compatibility::MCP,
        shared_formats: SharedFormats {
            setup: hive_setup_support::compatibility::FORMATS,
            comb_protocol: comb_protocol::compatibility::FORMATS,
        },
        compiled_features: &["http"],
    }
}

/// Pinned model provisioning and root-scoped setup.
pub mod provision;
