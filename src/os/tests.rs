//! Verify descriptor flags, polling bounds, file identity, and unreaped exit observation.

use super::fd::poll_timeout;
use super::*;
use rustix::{event::Timespec, fs::OFlags};
use std::os::unix::fs::MetadataExt;
use std::process::{Command, Stdio};
use std::time::Duration;

#[test]
fn capped_read_leaves_the_next_byte_unread_and_preserves_read_errors() {
    struct Failing;
    impl std::io::Read for Failing {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from_raw_os_error(5))
        }
    }
    let mut reader = std::io::Cursor::new(b"record+sentinel");
    assert_eq!(read_capped(&mut reader, 6).unwrap(), b"record");
    assert_eq!(reader.position(), 6);
    assert_eq!(read_capped(&mut reader, 0).unwrap(), b"");
    assert_eq!(reader.position(), 6);
    assert_eq!(read_capped(&mut reader, 20).unwrap(), b"+sentinel");
    assert_eq!(read_capped(Failing, 1).unwrap_err().raw_os_error(), Some(5));
}

#[test]
fn poll_timeout_rounds_up_and_caps_milliseconds() {
    let cap = Duration::from_millis(i32::MAX as u64);
    for (duration, seconds, nanoseconds) in [
        (Duration::ZERO, 0, 0),
        (Duration::from_nanos(1), 0, 1_000_000),
        (Duration::from_millis(1), 0, 1_000_000),
        (Duration::from_nanos(1_000_001), 0, 2_000_000),
        (Duration::from_nanos(999_999_999), 1, 0),
        (Duration::from_millis(1250), 1, 250_000_000),
        (
            cap.checked_sub(Duration::from_nanos(1)).unwrap(),
            2_147_483,
            647_000_000,
        ),
        (cap, 2_147_483, 647_000_000),
        (cap + Duration::from_nanos(1), 2_147_483, 647_000_000),
        (Duration::MAX, 2_147_483, 647_000_000),
    ] {
        assert_eq!(
            poll_timeout(duration),
            Timespec {
                tv_sec: seconds,
                tv_nsec: nanoseconds,
            },
            "{duration:?}"
        );
    }
}

#[test]
fn file_identity_matches_descriptor_metadata() {
    let file = tempfile::tempfile().unwrap();
    let metadata = file.metadata().unwrap();
    let identity = file_identity(&file).unwrap();
    assert_eq!(identity.device, metadata.dev());
    assert_eq!(identity.inode, metadata.ino());
}

#[test]
fn exit_observation_keeps_the_child_unreaped() {
    let mut child = crate::test_support::ChildGuard(
        spawn(
            Command::new("/bin/sh")
                .args(["-c", "read value; exit 23"])
                .stdin(Stdio::piped()),
        )
        .unwrap(),
    );
    assert!(!has_exited(&child).unwrap());
    child.stdin.take();
    crate::test_support::wait_until(Duration::from_secs(5), Duration::from_millis(1), || {
        has_exited(&child).unwrap().then_some(())
    });
    assert!(has_exited(&child).unwrap());
    assert!(has_exited(&child).unwrap());
    assert_eq!(child.wait().unwrap().code(), Some(23));
    assert_eq!(
        has_exited(&child).unwrap_err().raw_os_error(),
        Some(rustix::io::Errno::CHILD.raw_os_error())
    );
}

#[test]
fn descriptor_configuration_replaces_the_requested_flags() {
    let file = tempfile::tempfile().unwrap();
    rustix::io::fcntl_setfd(&file, rustix::io::FdFlags::CLOEXEC).unwrap();
    inherit_retained_fd(std::os::fd::AsRawFd::as_raw_fd(&file)).unwrap();
    assert_eq!(
        rustix::io::fcntl_getfd(&file).unwrap(),
        rustix::io::FdFlags::empty()
    );

    rustix::fs::fcntl_setfl(&file, OFlags::APPEND).unwrap();
    assert!(
        rustix::fs::fcntl_getfl(&file)
            .unwrap()
            .contains(OFlags::APPEND)
    );
    nonblocking(&file).unwrap();
    let flags = rustix::fs::fcntl_getfl(&file).unwrap();
    assert!(flags.contains(OFlags::NONBLOCK));
    assert!(!flags.contains(OFlags::APPEND));
    assert_eq!(flags & OFlags::ACCMODE, OFlags::RDWR);
}

#[test]
fn retained_callback_accepts_an_invalid_descriptor_without_borrowing_it() {
    assert_eq!(
        inherit_retained_fd(-1).unwrap_err().raw_os_error(),
        Some(rustix::io::Errno::BADF.raw_os_error())
    );
}

#[test]
fn a_terminal_hangs_up_once_its_other_side_closes() {
    use std::os::fd::{AsFd, FromRawFd, OwnedFd};
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: the outputs point to initialized integers; no name, settings or
    // size are requested.
    let opened = unsafe {
        libc::openpty(
            &raw mut master,
            &raw mut slave,
            std::ptr::null_mut(),
            // Mutable on macOS, const on Linux: a null *mut coerces to either.
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(opened, 0, "{}", std::io::Error::last_os_error());
    // SAFETY: a successful openpty transferred both uniquely owned descriptors.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    assert!(!terminal::hung_up(slave.as_fd()));
    drop(master);
    assert!(terminal::hung_up(slave.as_fd()));
}

#[cfg(target_os = "macos")]
#[test]
fn a_revoked_terminal_counts_as_hung_up() {
    use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
    unsafe extern "C" {
        /// BSD revoke(2), which the libc crate does not bind on macOS.
        fn revoke(path: *const libc::c_char) -> libc::c_int;
    }
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: the outputs point to initialized integers; no name, settings or
    // size are requested.
    let opened = unsafe {
        libc::openpty(
            &raw mut master,
            &raw mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(opened, 0, "{}", std::io::Error::last_os_error());
    // SAFETY: a successful openpty transferred both uniquely owned descriptors.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    // SAFETY: ptsname names the open master's terminal in static storage,
    // which is copied at once.
    let name = unsafe { std::ffi::CStr::from_ptr(libc::ptsname(master.as_raw_fd())) }.to_owned();
    assert!(!terminal::hung_up(slave.as_fd()));
    // Closing a window revokes its terminal: the descriptor becomes invalid
    // while the other side stays open.
    // SAFETY: revoke takes the NUL-terminated path of a terminal this test owns.
    assert_eq!(unsafe { revoke(name.as_ptr()) }, 0);
    assert!(terminal::hung_up(slave.as_fd()));
    drop(master);
}
