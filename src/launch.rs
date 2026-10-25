//! Exec a trusted shim, persist its birth identity, then authorize native exec.
use std::{mem, time::Duration};
// Persist authorization before the shim's later gate deadline expires.
const GRANT_TIMEOUT: Duration = Duration::from_secs(25);
const GATE_TIMEOUT: Duration = Duration::from_secs(30);
// Bound receipt of the native-exec status after sending the grant.
const STATUS_TIMEOUT: Duration = GATE_TIMEOUT;
const _: () = assert!(GRANT_TIMEOUT.as_secs() < GATE_TIMEOUT.as_secs());
// Native exec failures carry a fixed tag followed by one native-endian errno.
const STATUS_TAG: &[u8; 4] = b"EXEC";
const TAG_BYTES: usize = STATUS_TAG.len();
const STATUS_BYTES: usize = TAG_BYTES + mem::size_of::<i32>();
// One overflow byte distinguishes oversized status from the complete fixed record.
const STATUS_READ_BYTES: usize = STATUS_BYTES + 1;
const NATIVE_EXEC_FAILED: &str = "Native provider exec failed.";

mod cache;
mod gate;
mod image;
mod shim;

pub(crate) use cache::{LauncherCache, system_cache};
pub(crate) use gate::{spawn, spawn_with_terminal};
pub(crate) use shim::dispatch;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use tests::test_cache;
