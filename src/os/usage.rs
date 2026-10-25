//! Read-only CPU clocks. These functions never reap a child.
use std::io;

pub(crate) fn thread_ns() -> io::Result<u64> {
    let value = super::system::clock_time(libc::CLOCK_THREAD_CPUTIME_ID)?;
    u64::try_from(value.tv_sec)
        .map_err(io::Error::other)?
        .checked_mul(u64::from(super::NANOS_PER_SECOND))
        .and_then(|s| s.checked_add(u64::try_from(value.tv_nsec).ok()?))
        .ok_or_else(|| io::Error::other("Thread CPU clock overflow"))
}
