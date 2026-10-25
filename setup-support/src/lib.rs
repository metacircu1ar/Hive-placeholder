#![allow(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact the reviewed maintainer write inventory inventory"
)]
//! Private shared setup mechanics. This crate never depends on a provider or engine.
pub mod admission;
pub mod checkpoints;
pub mod compatibility;
pub mod directories;
pub mod environment;
pub mod files;
pub mod identity;
pub mod locks;
pub mod maintenance;
pub mod managed;
pub mod migration;
pub mod names;
pub mod process_images;
pub mod removal;
pub mod runner;
pub mod service;
#[cfg(test)]
mod tests;

pub type Result<T> = std::io::Result<T>;
pub fn invalid(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into())
}

#[derive(Debug)]
struct Context {
    action: &'static str,
    cause: std::io::Error,
}
impl std::fmt::Display for Context {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{}: {}", self.action, self.cause)
    }
}
impl std::error::Error for Context {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}
pub(crate) fn context(action: &'static str, cause: std::io::Error) -> std::io::Error {
    std::io::Error::new(cause.kind(), Context { action, cause })
}
