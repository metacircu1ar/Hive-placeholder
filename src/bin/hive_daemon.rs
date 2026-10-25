#![allow(
    clippy::disallowed_methods,
    reason = "Raw writes are gated by the exact the reviewed maintainer write inventory inventory"
)]
//! Standalone daemon process entry point.
fn main() {
    hive::run(true);
}
