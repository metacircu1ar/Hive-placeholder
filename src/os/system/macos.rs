//! Read macOS process birth identity and boot session bytes through narrow FFI calls.
// Accommodate the kernel's boot-session UUID text and terminating NUL.
use crate::{convert, os::no_such_process};
use std::{io, mem, ptr};
const BOOT_IDENTITY_BYTES: usize = 128;

#[cfg(target_os = "macos")]
pub(crate) struct ProcessBirth {
    pub(crate) seconds: u64,
    pub(crate) microseconds: u64,
}

#[cfg(target_os = "macos")]
// The caller validates the PID; preserve its C pid_t bit conversion at this FFI boundary.
#[expect(
    clippy::expect_used,
    reason = "The fixed proc_bsdinfo ABI structure size fits the signed C API length."
)]
pub(crate) fn process_birth(pid: u32) -> io::Result<Option<ProcessBirth>> {
    // SAFETY: proc_bsdinfo is a plain C struct whose all-zero value is valid.
    let mut info: libc::proc_bsdinfo = unsafe { mem::zeroed() };
    let size =
        i32::try_from(mem::size_of_val(&info)).expect("proc_bsdinfo size fits the C API length");
    // SAFETY: info is writable for exactly size bytes; the PID is validated by
    // ProcessIdentity::read, and PROC_PIDTBSDINFO expects this struct layout.
    let read = unsafe {
        libc::proc_pidinfo(
            convert::bits_i32(pid),
            libc::PROC_PIDTBSDINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    if read == size {
        return Ok(Some(ProcessBirth {
            seconds: info.pbi_start_tvsec,
            microseconds: info.pbi_start_tvusec,
        }));
    }
    let error = io::Error::last_os_error();
    if read == 0 && no_such_process(&error) {
        Ok(None)
    } else {
        Err(error)
    }
}

#[cfg(target_os = "macos")]
pub(crate) struct BootIdentityBytes {
    pub(crate) bytes: [u8; BOOT_IDENTITY_BYTES],
    pub(crate) length: usize,
}

#[cfg(target_os = "macos")]
pub(crate) fn boot_identity_bytes() -> io::Result<BootIdentityBytes> {
    let mut boot = [0_u8; BOOT_IDENTITY_BYTES];
    let mut size = boot.len();
    // SAFETY: The name is NUL-terminated; boot/size are valid writable output
    // buffers, and the null new-value pointer requests a read without mutation.
    let result = unsafe {
        libc::sysctlbyname(
            c"kern.bootsessionuuid".as_ptr(),
            boot.as_mut_ptr().cast(),
            &raw mut size,
            ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(BootIdentityBytes {
        bytes: boot,
        length: size,
    })
}
