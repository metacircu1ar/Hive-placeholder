//! Bind without admitting connections until the lease establishes private mode.
use rustix::net::{self, AddressFamily, SocketAddrUnix, SocketType};
use std::{io, os::unix::net::UnixListener, path::Path};

pub(super) fn bound(path: &Path) -> io::Result<UnixListener> {
    #[cfg(target_os = "linux")]
    let fd = net::socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        net::SocketFlags::CLOEXEC,
        None,
    )?;
    #[cfg(not(target_os = "linux"))]
    let fd = {
        let fd = net::socket(AddressFamily::UNIX, SocketType::STREAM, None)?;
        rustix::io::fcntl_setfd(&fd, rustix::io::FdFlags::CLOEXEC)?;
        #[cfg(target_vendor = "apple")]
        net::sockopt::set_socket_nosigpipe(&fd, true)?;
        fd
    };
    net::bind(&fd, &SocketAddrUnix::new(path)?)?;
    Ok(fd.into())
}

pub(super) fn listen(listener: &UnixListener) -> io::Result<()> {
    // Match std's backlog on the supported Linux and macOS platforms.
    net::listen(listener, -1)?;
    Ok(())
}
