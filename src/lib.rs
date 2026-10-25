#![allow(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact the reviewed maintainer write inventory inventory"
)]
//! `Hive` command, provider and runtime library entry points.
#![cfg(unix)]

#[cfg(test)]
extern crate self as hive;

mod approvals;
mod auth;
mod calls;
mod cli;
pub mod compatibility;
pub mod config;
mod convert;
pub mod error;
mod executable;
mod external_mcp;
mod launch;
mod limits;
mod observability;
pub mod options;
mod os;
mod process;
mod provider_support;
mod publication;
pub mod runtime;
mod shell;
pub mod state;

/// Dispatch trusted process modes, then run the CLI or standalone daemon.
pub fn run(daemon: bool) {
    executable::capture();
    launch::dispatch();
    shell::dispatch();
    // Owned process groups are proven absent by this process; their orphans
    // must come back here to be reaped, never wait on a container's PID 1.
    #[cfg(target_os = "linux")]
    os::adopt_orphans();
    cli::run(daemon);
}

#[cfg(test)]
mod test_support;

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod integration_support;
