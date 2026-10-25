//! Prepare bounded caller input before knowledge or connection setup.
mod context;
mod credential;
mod embedding;
mod failure;
mod hints;
mod paths;
mod reader;
mod remote;
pub(super) mod setup;
#[cfg(test)]
mod tests;

pub(super) use context::Context;
pub(crate) use failure::Failure;
pub(super) use hints::Exclusive;
pub(super) type Result<T> = std::result::Result<T, Failure>;
