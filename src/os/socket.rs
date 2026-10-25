//! Validate connected shell socket identities with platform-specific kernel evidence.
// The native peer verifier returns one only for a confirmed socket identity.
#[cfg(target_os = "macos")]
const PEER_MATCH: i32 = 1;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
use super::invalid;
#[cfg(target_os = "linux")]
pub(crate) use linux::{connect_inherited, reuse_unconnected, unconnected_socket};
#[cfg(target_os = "macos")]
use std::os::fd::AsRawFd;
use std::{io, os::unix::net::UnixStream};

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn hive_socket_peer(
        peer: libc::pid_t,
        remote_fd: libc::c_int,
        local_fd: libc::c_int,
    ) -> libc::c_int;
}

#[cfg_attr(
    target_os = "linux",
    expect(
        clippy::unnecessary_wraps,
        reason = "The shared guard propagates Darwin's fallible live socket verification."
    )
)]
pub(crate) fn socket_peer(stream: &UnixStream, pid: u32, remote_fd: i32) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        if remote_fd < 0 {
            return Err(invalid());
        }
        let pid = i32::try_from(pid).map_err(|_| invalid())?;
        // SAFETY: wrapper accepts scalar IDs and uses SDK-sized kernel output
        // structures. Invalid remote descriptors return failure without borrowing.
        if unsafe { hive_socket_peer(pid, remote_fd, stream.as_raw_fd()) } == PEER_MATCH {
            Ok(())
        } else {
            Err(invalid())
        }
    }
    #[cfg(target_os = "linux")]
    {
        // Linux SO_PEERCRED is captured at connect, unlike Darwin's live lookup.
        let _ = (stream, pid, remote_fd);
        Ok(())
    }
}
