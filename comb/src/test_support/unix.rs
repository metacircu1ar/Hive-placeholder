use std::{ffi::CString, io, os::unix::ffi::OsStrExt, path::Path};

pub(crate) fn fifo(path: &Path) -> io::Result<()> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|cause| io::Error::new(io::ErrorKind::InvalidInput, cause))?;
    // SAFETY: path owns a live NUL-terminated byte string for this call; mkfifo
    // does not retain its pointer. Mode 0o666 is filtered by the unchanged umask.
    if unsafe { libc::mkfifo(path.as_ptr(), 0o666) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
