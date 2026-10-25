use std::{fs::File, io, os::fd::AsRawFd};

pub(super) fn ordering_barrier(file: &File) -> io::Result<()> {
    // SAFETY: the descriptor is owned and live; F_BARRIERFSYNC takes no
    // pointer argument. This orders temp data before the later rename.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_BARRIERFSYNC) } == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
