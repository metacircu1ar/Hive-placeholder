//! Contextual diagnostics around the native mutation envelopes and value types.
mod convert;
mod metadata;
mod shape;
#[cfg(test)]
mod tests;
pub(crate) use convert::operation;
pub use metadata::Write;
