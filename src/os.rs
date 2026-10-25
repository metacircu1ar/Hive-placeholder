//! Wrap OS and FFI calls while leaving ownership, retries, and lifecycle policy with callers.
// State and helper paths admit no group/other access.
pub(crate) const PRIVATE_ACCESS_MASK: u32 = 0o077;
pub(crate) const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
pub(crate) const PRIVATE_FILE_MODE: u32 = 0o600;
// Immutable executable images remain readable/executable only by their owner.
pub(crate) const READ_EXECUTE_MODE: u32 = 0o500;
pub(crate) const EXECUTABLE_BITS: u32 = 0o111;
// Clock conversion factors shared by FFI clocks and poll timeout rounding.
const MILLIS_PER_SECOND: u32 = 1000;
const NANOS_PER_MILLISECOND: u32 = 1_000_000;
const NANOS_PER_SECOND: u32 = MILLIS_PER_SECOND * NANOS_PER_MILLISECOND;
// poll_readable supervises input, output and error descriptors of one child.
const CAPTURE_DESCRIPTOR_COUNT: usize = 3;

mod fd;
mod group;
#[cfg(target_os = "macos")]
pub(crate) mod keychain;
mod peer;
mod process;
#[cfg(target_os = "linux")]
mod reaper;
pub(crate) mod shell;
mod socket;
mod system;
pub(crate) mod terminal;
pub(crate) mod usage;

pub(crate) use fd::{
    FileIdentity, append_mode, close_on_exec, descriptors_exhausted, exclusive_lock, file_identity,
    inherit_retained_fd, nonblocking, poll_many, poll_readable, poll_writable, private_file,
    private_reader, read_capped, read_text_capped, shared_lock,
};
#[cfg(target_os = "linux")]
pub(crate) use group::adopt_orphans;
pub(crate) use group::{group_absent, own_process_group, signal_group};
pub(crate) use process::{
    configure_launcher, configure_process_group, fork_guard, has_exited, install_cancellation,
    new_session, no_such_process, output, reset_sigchld, signal_child, spawn, take_launcher_gate,
};
#[cfg(target_os = "linux")]
pub(crate) use reaper::{reap_adopted, reap_group, reap_process, register as register_child};
pub(crate) use rustix::process::Signal;
#[cfg(target_os = "macos")]
pub(crate) use system::{BootIdentityBytes, ProcessBirth, boot_identity_bytes, process_birth};
pub(crate) use system::{
    SqliteMemory, diagnostic_clock_ns, effective_uid, private_umask, sqlite_memory, username,
};

use std::io;

// Orphans reach launchd on macOS, which always reaps them.
#[cfg(not(target_os = "linux"))]
pub(crate) fn register_child(_pid: u32) {}
#[cfg(not(target_os = "linux"))]
#[expect(clippy::unnecessary_wraps, reason = "Linux reaping can fail.")]
pub(crate) fn reap_group(_group: u32) -> io::Result<()> {
    Ok(())
}
#[cfg(not(target_os = "linux"))]
#[expect(clippy::unnecessary_wraps, reason = "Linux reaping can fail.")]
pub(crate) fn reap_process(_pid: u32, _start: u64) -> io::Result<()> {
    Ok(())
}

/// Ordinary path selection failures can be corrected by the caller. Resource and
/// storage failures such as EMFILE/EIO remain operational failures.
pub(crate) fn input_path_error(cause: &io::Error) -> bool {
    matches!(
        cause.kind(),
        io::ErrorKind::NotFound
            | io::ErrorKind::PermissionDenied
            | io::ErrorKind::InvalidInput
            | io::ErrorKind::NotADirectory
    ) || matches!(
        cause.raw_os_error(),
        Some(libc::ELOOP | libc::ENAMETOOLONG | libc::ENXIO | libc::ENODEV | libc::EOPNOTSUPP)
    )
}

// Native numeric/proc validation deliberately returns raw EINVAL. io::Error cannot
// retain a separate parse/conversion source alongside an errno; callers rely on
// that errno, so map_err(|_| invalid()) drops only that lower-level validation detail.
fn invalid() -> io::Error {
    io::Error::from_raw_os_error(libc::EINVAL)
}

#[cfg(test)]
mod tests;
