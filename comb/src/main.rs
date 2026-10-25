#![allow(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact the reviewed maintainer write inventory inventory"
)]
//! Human command-line interface; service routing is shared with MCP clients.
mod cli;

use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = match cli::parsing::read() {
        Ok(cli) => cli,
        Err(output) => return ExitCode::from(cli::parsing::emit(&output)),
    };
    let mcp = cli.is_mcp();
    ExitCode::from(cli::output::finish(cli::run(cli), mcp))
}
