//! Create and validate inherited Linux shell sockets before sandboxed connection.
use crate::os::{fd::close_retained_on_exec, inherit_retained_fd, invalid};
use std::{
    io, mem,
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::Path,
};

/// Created before Linux's sandbox filter, connected by the actual inner helper.
/// This permits only the owned descriptor; `AF_UNIX` socket creation stays blocked.
#[cfg(target_os = "linux")]
pub(crate) fn unconnected_socket() -> io::Result<OwnedFd> {
    use std::os::fd::FromRawFd;
    // SAFETY: socket accepts scalar arguments; returned descriptor is newly owned.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful socket call transferred this fresh descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}
#[cfg(target_os = "linux")]
pub(crate) fn connect_inherited(fd: i32, path: &Path) -> io::Result<UnixStream> {
    use std::os::{fd::FromRawFd, unix::ffi::OsStrExt};
    if fd <= libc::STDERR_FILENO {
        return Err(invalid());
    }
    close_retained_on_exec(fd)?;
    // SAFETY: this private executable receives sole ownership of the shim's inherited socket.
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    // SAFETY: sockaddr_un is a plain C structure, initialized before use.
    let mut address: libc::sockaddr_un = unsafe { mem::zeroed() };
    let bytes = path.as_os_str().as_bytes();
    if bytes.contains(&0) || bytes.len() >= address.sun_path.len() {
        return Err(invalid());
    }
    address.sun_family = libc::sa_family_t::try_from(libc::AF_UNIX).map_err(|_| invalid())?;
    for (to, from) in address.sun_path.iter_mut().zip(bytes) {
        *to = libc::c_char::from_ne_bytes([*from]);
    }
    let length = libc::socklen_t::try_from(mem::size_of_val(&address)).map_err(|_| invalid())?;
    // SAFETY: address is initialized and valid for the supplied structure size.
    if unsafe { libc::connect(fd, (&raw const address).cast(), length) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(stream)
}

#[cfg(target_os = "linux")]
pub(crate) fn reuse_unconnected(fd: i32) -> io::Result<OwnedFd> {
    use std::os::fd::FromRawFd;
    if fd <= libc::STDERR_FILENO {
        return Err(invalid());
    }
    let mut kind: libc::c_int = 0;
    let mut length = libc::socklen_t::try_from(mem::size_of_val(&kind)).map_err(|_| invalid())?;
    // SAFETY: SO_TYPE accepts even invalid descriptors and writes one c_int.
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&raw mut kind).cast(),
            &raw mut length,
        )
    } != 0
        || kind != libc::SOCK_STREAM
    {
        return Err(invalid());
    }
    // SAFETY: plain C output structure, valid as a zero-initialized buffer.
    let mut address: libc::sockaddr_un = unsafe { mem::zeroed() };
    length = libc::socklen_t::try_from(mem::size_of_val(&address)).map_err(|_| invalid())?;
    // SAFETY: getsockname writes to the buffer of the supplied size.
    if unsafe { libc::getsockname(fd, (&raw mut address).cast(), &raw mut length) } != 0
        || i32::from(address.sun_family) != libc::AF_UNIX
    {
        return Err(invalid());
    }
    length = libc::socklen_t::try_from(mem::size_of_val(&address)).map_err(|_| invalid())?;
    // SAFETY: getpeername writes to the buffer; unconnected is required here.
    if unsafe { libc::getpeername(fd, (&raw mut address).cast(), &raw mut length) } == 0
        || io::Error::last_os_error().raw_os_error() != Some(libc::ENOTCONN)
    {
        return Err(invalid());
    }
    inherit_retained_fd(fd)?;
    // SAFETY: validated descriptor is transferred through this private shim's exec environment.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        ffi::OsStr,
        io::{Read, Write},
        os::unix::net::UnixListener,
        os::{fd::IntoRawFd, unix::ffi::OsStrExt},
    };

    #[test]
    fn inherited_socket_preserves_non_utf8_path_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(OsStr::from_bytes(b"socket-\xff"));
        let listener = UnixListener::bind(&path).unwrap();
        let fd = unconnected_socket().unwrap().into_raw_fd();
        let mut client = connect_inherited(fd, &path).unwrap();
        client.write_all(b"ping").unwrap();
        let (mut server, _) = listener.accept().unwrap();
        let mut message = [0; 4];
        server.read_exact(&mut message).unwrap();
        assert_eq!(&message, b"ping");
    }
}
