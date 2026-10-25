//! Configure owned descriptors, poll readiness, lock files, and verify file identity.
use crate::{convert, os};
use rustix::{
    event::{self, PollFd, PollFlags, Timespec},
    fs::{self as native_fs, FlockOperation, OFlags},
    io::Errno,
};
use std::{
    array,
    fs::{File, OpenOptions},
    io::{self, Read},
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, RawFd},
        unix::fs::OpenOptionsExt,
    },
    path::Path,
    time::Duration,
};

// These wrappers are used after fork: only async-signal-safe syscalls and
// conversion of an errno to io::Error (no strings, allocation, or locking).
/// Descriptor inheritance for auth's retained `&mut Command` callback. The operation owns
/// fd during its first spawn, but the caller may later reuse the Command after
/// finishing that operation. An invalid fd must still return EBADF, so creating
/// a `BorrowedFd` for rustix would be unsound. Consuming Command changes the API;
/// retaining its lock would extend ownership beyond `operation.finish()`.
pub(crate) fn inherit_retained_fd(fd: RawFd) -> io::Result<()> {
    // SAFETY: F_SETFD takes an integer argument and no pointers. Any fd integer
    // is accepted by the syscall, including a closed descriptor that yields EBADF.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, 0) } == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Replace the descriptor flags after the caller has finished inheriting the descriptor.
pub(crate) fn close_on_exec(fd: impl AsFd) -> io::Result<()> {
    close_retained_on_exec(fd.as_fd().as_raw_fd())
}

pub(super) fn close_retained_on_exec(fd: RawFd) -> io::Result<()> {
    // SAFETY: F_SETFD accepts any integer descriptor and no pointers, including
    // a closed descriptor that must return EBADF before an owner is constructed.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(crate) fn nonblocking(fd: impl AsFd) -> io::Result<()> {
    // Replace the flags exactly; do not merge in previously set status flags.
    native_fs::fcntl_setfl(fd, OFlags::NONBLOCK).map_err(Into::into)
}

#[expect(
    clippy::expect_used,
    reason = "Milliseconds are capped at i32::MAX before conversion to i64."
)]
pub(super) fn poll_timeout(timeout: Duration) -> Timespec {
    // Round up so conversion cannot shorten a wait or make a positive timeout
    // a busy poll. Cap at the signed millisecond limit accepted by poll.
    let milliseconds = i64::try_from(
        timeout
            .as_nanos()
            .div_ceil(u128::from(super::NANOS_PER_MILLISECOND))
            .min(i32::MAX as u128),
    )
    .expect("poll milliseconds are capped at i32::MAX");
    Timespec {
        tv_sec: milliseconds / i64::from(super::MILLIS_PER_SECOND),
        tv_nsec: (milliseconds % i64::from(super::MILLIS_PER_SECOND)
            * i64::from(super::NANOS_PER_MILLISECOND)) as _,
    }
}

pub(crate) fn poll_readable(
    descriptors: [Option<BorrowedFd<'_>>; super::CAPTURE_DESCRIPTOR_COUNT],
    timeout: Duration,
) -> io::Result<()> {
    let timeout = poll_timeout(timeout);
    let mut active = descriptors.into_iter().flatten();
    let result = if let Some(first) = active.next() {
        let mut fds = array::from_fn::<_, { super::CAPTURE_DESCRIPTOR_COUNT }, _>(|_| {
            PollFd::from_borrowed_fd(first, PollFlags::IN)
        });
        let mut count = 1;
        for fd in active {
            fds[count] = PollFd::from_borrowed_fd(fd, PollFlags::IN);
            count += 1;
        }
        event::poll(&mut fds[..count], Some(&timeout))
    } else {
        event::poll(&mut [], Some(&timeout))
    };
    match result {
        Ok(_) | Err(Errno::INTR) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn poll_writable(descriptor: BorrowedFd<'_>, timeout: Duration) -> io::Result<()> {
    let mut fds = [PollFd::from_borrowed_fd(descriptor, PollFlags::OUT)];
    let timeout = poll_timeout(timeout);
    match event::poll(&mut fds, Some(&timeout)) {
        Ok(_) | Err(Errno::INTR) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn poll_many(descriptors: &[BorrowedFd<'_>], timeout: Duration) -> io::Result<()> {
    let mut fds: Vec<_> = descriptors
        .iter()
        .map(|fd| PollFd::from_borrowed_fd(*fd, PollFlags::IN))
        .collect();
    let timeout = poll_timeout(timeout);
    match event::poll(&mut fds, Some(&timeout)) {
        Ok(_) | Err(Errno::INTR) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

pub(crate) fn shared_lock(fd: impl AsFd, nonblocking: bool) -> io::Result<()> {
    let operation = if nonblocking {
        FlockOperation::NonBlockingLockShared
    } else {
        FlockOperation::LockShared
    };
    native_fs::flock(fd, operation).map_err(Into::into)
}

pub(crate) fn exclusive_lock(fd: impl AsFd, nonblocking: bool) -> io::Result<()> {
    let operation = if nonblocking {
        FlockOperation::NonBlockingLockExclusive
    } else {
        FlockOperation::LockExclusive
    };
    native_fs::flock(fd, operation).map_err(Into::into)
}

pub(crate) struct FileIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
}

// Match MetadataExt::dev(): macOS dev_t is signed, so negative identifiers widen
// to u64 modulo 2^64. Descriptor and path metadata must give the same identity.
#[allow(clippy::cast_sign_loss)]
pub(crate) fn file_identity(fd: impl AsFd) -> io::Result<FileIdentity> {
    let stat = native_fs::fstat(fd)?;
    Ok(FileIdentity {
        device: stat.st_dev as u64,
        inode: stat.st_ino as u64,
    })
}

// OpenOptions takes signed C flags; preserve the rustix flag bit pattern unchanged.
pub(crate) fn private_file(path: &Path, create: bool) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .mode(os::PRIVATE_FILE_MODE)
        .custom_flags(convert::bits_i32(
            (OFlags::NOFOLLOW | OFlags::CLOEXEC).bits(),
        ))
        .open(path)
}
pub(crate) fn append_mode(file: &File) -> io::Result<()> {
    native_fs::fcntl_setfl(file, native_fs::fcntl_getfl(file)? | OFlags::APPEND)?;
    Ok(())
}
pub(crate) fn private_reader(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(convert::bits_i32(
            (OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK).bits(),
        ))
        .open(path)
}

pub(crate) fn descriptors_exhausted(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::EMFILE | libc::ENFILE))
}

/// Read at most the caller's cap without changing its file-opening or overflow policy.
pub(crate) fn read_capped(reader: impl Read, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Text counterpart of `read_capped`; preserve Read's UTF-8 and I/O error behavior.
pub(crate) fn read_text_capped(reader: impl Read, limit: u64) -> io::Result<String> {
    let mut text = String::new();
    reader.take(limit).read_to_string(&mut text)?;
    Ok(text)
}
