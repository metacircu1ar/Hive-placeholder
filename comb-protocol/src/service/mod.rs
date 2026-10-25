//! Shared wire protocol, socket client and MCP catalog.
pub mod api;
pub mod auth;
pub mod client;
pub mod framing;
pub mod input;
pub mod limits;
pub mod mcp;
use crate::{Error, Result};
pub use api::Operation;
use std::{
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::Path,
};
pub fn check_socket(path: &Path) -> Result<()> {
    let m = std::fs::symlink_metadata(path)?;
    if !m.file_type().is_socket()
        || m.uid() != rustix::process::geteuid().as_raw()
        || m.mode() & 0o077 != 0
    {
        return Err(Error::unsafe_socket());
    }
    Ok(())
}
