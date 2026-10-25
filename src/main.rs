#![allow(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact the reviewed maintainer write inventory inventory"
)]
//! Command-line entry point and process dispatch.
fn main() {
    hive::run(false);
}
