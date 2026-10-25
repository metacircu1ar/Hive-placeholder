//! One bounded source read. Pipe input has one consumer; descriptor flags are untouched.
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read},
    os::unix::fs::{FileTypeExt, OpenOptionsExt},
    path::Path,
    time::{Duration, Instant},
};

pub(super) const PIPE_DEADLINE: Duration = Duration::from_secs(30);

pub(super) fn read(path: &Path, cap: usize, timeout: Duration) -> io::Result<Vec<u8>> {
    source(path, cap, timeout).map(|source| source.bytes)
}
pub(super) struct Source {
    pub bytes: Vec<u8>,
    pub regular: bool,
}
pub(super) fn source(path: &Path, cap: usize, timeout: Duration) -> io::Result<Source> {
    let deadline = Instant::now() + timeout;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    let kind = file.metadata()?.file_type();
    if kind.is_file() {
        let mut bytes = Vec::new();
        file.take(cap as u64 + 1).read_to_end(&mut bytes)?;
        return Ok(Source {
            bytes,
            regular: true,
        });
    }
    if !kind.is_fifo() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Input must be a regular file or pipe.",
        ));
    }
    Ok(Source {
        bytes: pipe(file, cap, deadline)?,
        regular: false,
    })
}

fn pipe(mut file: File, cap: usize, deadline: Instant) -> io::Result<Vec<u8>> {
    // macOS FIFO poll may omit EOF after data was consumed. Probe only a
    // descriptor already known to be nonblocking, never a shared blocking fd.
    let nonblocking = rustix::fs::fcntl_getfl(&file)?.contains(rustix::fs::OFlags::NONBLOCK);
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let probe_eof = nonblocking && !bytes.is_empty();
        let interval = if probe_eof {
            remaining.min(INPUT_POLL)
        } else {
            remaining
        };
        if !ready(&file, interval)? && !probe_eof {
            continue;
        }
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        // /dev/fd can duplicate a blocking open description. Read it only on
        // readiness, with one consumer, without mutating its descriptor flags.
        let take = buffer.len().min(cap + 1 - bytes.len());
        match file.read(&mut buffer[..take]) {
            Ok(0) => return Ok(bytes),
            Ok(count) => bytes.extend_from_slice(&buffer[..count]),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        }
        if bytes.len() > cap {
            return Ok(bytes);
        }
    }
}

fn ready(file: &File, duration: Duration) -> io::Result<bool> {
    let timeout = Timespec::try_from(duration).map_err(io::Error::other)?;
    let mut fds = [PollFd::new(file, PollFlags::IN)];
    match poll(&mut fds, Some(&timeout)) {
        Ok(0) | Err(rustix::io::Errno::INTR) => return Ok(false),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let events = fds[0].revents();
    if events.contains(PollFlags::NVAL) {
        return Err(io::Error::other("Invalid input descriptor."));
    }
    if !events.intersects(PollFlags::IN | PollFlags::HUP) {
        return Err(io::Error::other("Input stream readiness failed."));
    }
    Ok(true)
}

const INPUT_POLL: std::time::Duration = Duration::from_millis(20);
