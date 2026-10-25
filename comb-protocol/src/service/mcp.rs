//! Thin stdio MCP client and the hosted tools' common catalog/parser.
mod arguments;
mod catalog;
mod errors;
mod protocol;
mod session;

pub use arguments::operation;
pub use catalog::{Definition, tools};
pub use errors::tool_error;
pub use session::run;
