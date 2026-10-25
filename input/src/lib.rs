//! Shared, side-effect-free input checks and explicit public error views.
//!
//! Callers supply trusted metadata and examples, never raw parser/provider errors.
//! Received values are omitted unless an adapter explicitly admits public input.
//! This crate does not open files, connect sockets or access process state.
pub mod compatibility;
mod error;
mod examples;
mod render;
pub mod validate;

#[cfg(feature = "clap")]
pub mod clap;

pub use error::{Code, InputError, InvalidMetadata, WireError};
pub use examples::Examples;
pub use render::{Product, Rendered};

#[cfg(test)]
mod tests;
