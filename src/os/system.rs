//! Read system identity and diagnostic clocks and memory without changing caller policy.
use super::output;
use crate::{
    error::{Error, Result},
    state,
};
use rusqlite::{Connection, ffi};
use rustix::{fs as native_fs, process as native_process};
use std::{io, process::Command};
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::{BootIdentityBytes, ProcessBirth, boot_identity_bytes, process_birth};

pub(crate) fn effective_uid() -> u32 {
    native_process::geteuid().as_raw()
}

pub(crate) fn private_umask() {
    native_process::umask(native_fs::Mode::RWXG | native_fs::Mode::RWXO);
}

pub(crate) struct SqliteMemory {
    pub(crate) allocated: i64,
    pub(crate) cache: i32,
}

// rusqlite has no safe connection-cache status API. Keep this small foreign
// function boundary here alongside the OS FFI rather than exposing a DB handle.
pub(crate) fn sqlite_memory(connection: &Connection) -> SqliteMemory {
    let mut cache = 0;
    let mut high = 0;
    // SAFETY: the borrow keeps the connection live; its handle never escapes.
    let handle = unsafe { connection.handle() };
    // SAFETY: Connection is not Sync; both output counters are valid, and
    // reset=0 leaves counters intact.
    unsafe {
        ffi::sqlite3_db_status(
            handle,
            ffi::SQLITE_DBSTATUS_CACHE_USED,
            &raw mut cache,
            &raw mut high,
            0,
        );
    }
    // SAFETY: SQLite's process memory query takes no arguments or ownership.
    let allocated = unsafe { ffi::sqlite3_memory_used() };
    SqliteMemory { allocated, cache }
}

pub(crate) fn username() -> Result<String> {
    let output = output(Command::new("/usr/bin/id").arg("-un")).map_err(Error::state_io_from)?;
    if !output.status.success() {
        return Err(state::recovery_required());
    }
    // Keep the UTF-8 position without retaining the captured username bytes.
    let name = String::from_utf8(output.stdout)
        .map_err(|cause| state::recovery_required().with_source(cause.utf8_error()))?;
    Ok(name.trim_end_matches('\n').into())
}

/// Same elapsed-time clock used by Python's monotonic clock on our supported hosts.
pub(crate) fn diagnostic_clock_ns() -> io::Result<u64> {
    #[cfg(target_os = "macos")]
    let clock = libc::CLOCK_UPTIME_RAW;
    #[cfg(not(target_os = "macos"))]
    let clock = libc::CLOCK_MONOTONIC;
    let value = clock_time(clock)?;
    let seconds = u64::try_from(value.tv_sec).map_err(io::Error::other)?;
    let nanos = u64::try_from(value.tv_nsec).map_err(io::Error::other)?;
    seconds
        .checked_mul(u64::from(super::NANOS_PER_SECOND))
        .and_then(|s| s.checked_add(nanos))
        .ok_or_else(|| io::Error::other("Monotonic clock overflow"))
}

/// Read a native timespec; each caller retains its clock-specific conversion errors.
pub(super) fn clock_time(clock: libc::clockid_t) -> io::Result<libc::timespec> {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: value is writable for a complete timespec; clock is a valid OS constant.
    if unsafe { libc::clock_gettime(clock, &raw mut value) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(value)
}
