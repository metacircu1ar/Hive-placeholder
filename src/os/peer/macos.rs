//! Read macOS shell peer ancestry and executable identity through kernel APIs.
// proc_pidpath's fixed output bound includes the terminating NUL.
use super::invalid;
use std::{ffi::OsStr, io, mem, path::PathBuf};
const EXECUTABLE_PATH_BYTES: u32 = 4096;

#[cfg(target_os = "macos")]
pub(super) fn info(pid: u32) -> io::Result<libc::proc_bsdinfo> {
    // SAFETY: proc_bsdinfo is a plain C output structure.
    let mut info: libc::proc_bsdinfo = unsafe { mem::zeroed() };
    let size = i32::try_from(mem::size_of_val(&info)).map_err(|_| invalid())?;
    // SAFETY: buffer and length match PROC_PIDTBSDINFO's ABI; pid is range-checked.
    let count = unsafe {
        libc::proc_pidinfo(
            i32::try_from(pid).map_err(|_| invalid())?,
            libc::PROC_PIDTBSDINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    if count == size {
        Ok(info)
    } else {
        Err(io::Error::last_os_error())
    }
}
#[cfg(target_os = "macos")]
pub(super) fn executable(pid: u32) -> io::Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut buffer = [0_u8; EXECUTABLE_PATH_BYTES as usize];
    // SAFETY: proc_pidpath writes at most the supplied size to the valid buffer.
    let count = unsafe {
        libc::proc_pidpath(
            i32::try_from(pid).map_err(|_| invalid())?,
            buffer.as_mut_ptr().cast(),
            EXECUTABLE_PATH_BYTES,
        )
    };
    if count <= 0 {
        return Err(io::Error::last_os_error());
    }
    let end = buffer.iter().position(|b| *b == 0).ok_or_else(invalid)?;
    Ok(PathBuf::from(OsStr::from_bytes(&buffer[..end])))
}
