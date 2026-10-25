//! Private controlling terminal for native login; the CLI retains its foreground
//! terminal so cancellation can always supervise the owned native process group.
// Keep the existing ttyname_r bound, including its terminating NUL.
use std::{
    ffi::CStr,
    fs::{File, OpenOptions},
    io, mem,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::OpenOptionsExt,
    },
    ptr,
};
const TERMINAL_PATH_BYTES: usize = 4096;

pub(crate) struct Terminal {
    pub(crate) master: File,
    pub(crate) slave: File,
    pub(crate) output: File,
    pub(crate) input: File,
    original: libc::termios,
}
impl Terminal {
    pub(crate) fn open() -> io::Result<Self> {
        let _fork = super::fork_guard()?;
        let output = terminal_stream(libc::STDOUT_FILENO, false)?;
        let input = terminal_stream(libc::STDIN_FILENO, true)?;
        let (master, slave, original) = open_pair()?;
        let terminal = Self {
            master,
            slave,
            output,
            input,
            original,
        };
        for fd in [&terminal.master, &terminal.slave] {
            super::close_on_exec(fd)?;
        }
        super::nonblocking(&terminal.master)?;
        let mode = raw_mode(original);
        // SAFETY: mode is initialized and borrowed for this synchronous terminal update.
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw const mode) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(terminal)
    }
    pub(crate) fn input_ready(&self) -> io::Result<bool> {
        let mut descriptor = libc::pollfd {
            fd: self.input.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll gets one initialized descriptor and never retains it.
        let result = unsafe { libc::poll(&raw mut descriptor, 1, 0) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                return Ok(false);
            }
            return Err(error);
        }
        // macOS returns POLLNVAL for the controlling-terminal alias /dev/tty
        // even though nonblocking read works. The owned File is still valid;
        // let the relay attempt read and handle WouldBlock or terminal closure.
        Ok(
            descriptor.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL)
                != 0,
        )
    }
}
/// Whether this terminal's other side has closed: a closed window, a killed
/// multiplexer session or a dropped connection. Hangup and error count, and so
/// does an invalid descriptor that is no longer a terminal: macOS revokes a
/// closed window's terminal that way. The live `/dev/tty` alias also reports
/// POLLNVAL on macOS, but it is still a terminal.
pub(crate) fn hung_up(fd: std::os::fd::BorrowedFd<'_>) -> bool {
    let mut descriptor = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: poll gets one initialized descriptor and never retains it.
    let ready = unsafe { libc::poll(&raw mut descriptor, 1, 0) };
    if ready <= 0 {
        return false;
    }
    if descriptor.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
        return true;
    }
    // SAFETY: isatty only inspects the borrowed, open descriptor.
    descriptor.revents & libc::POLLNVAL != 0 && unsafe { libc::isatty(fd.as_raw_fd()) } == 0
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // SAFETY: original terminal attributes came from tcgetattr on stdin;
        // the foreground CLI restores them on all normal/error return paths.
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw const self.original);
        }
    }
}
pub(crate) fn control_stdin() -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let request = libc::c_ulong::from(libc::TIOCSCTTY);
    #[cfg(target_os = "linux")]
    let request = libc::TIOCSCTTY;
    // SAFETY: runs after setsid in the child; fd0 is its owned PTY slave.
    if unsafe { libc::ioctl(libc::STDIN_FILENO, request, 0) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: getpgrp reads the calling process group and takes no pointers.
    let group = unsafe { libc::getpgrp() };
    // SAFETY: tcsetpgrp takes integer arguments; fd0 is the owned PTY slave.
    if unsafe { libc::tcsetpgrp(libc::STDIN_FILENO, group) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn terminal_stream(fd: i32, input: bool) -> io::Result<File> {
    let mut path = [0 as libc::c_char; TERMINAL_PATH_BYTES];
    // SAFETY: ttyname_r writes at most the provided buffer length and stores a
    // nul-terminated path on success. No borrowed pointer escapes this call.
    let result = unsafe { libc::ttyname_r(fd, path.as_mut_ptr(), path.len()) };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result));
    }
    // SAFETY: the successful ttyname_r call initialized a nul-terminated path.
    // Preserve raw EINVAL for malformed native pathname bytes; wrapping the UTF-8
    // cause would remove the errno that callers use to classify OS failures.
    let text = unsafe { CStr::from_ptr(path.as_ptr()) }
        .to_str()
        .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
    // Fresh descriptions avoid changing inherited status flags. Input must be
    // nonblocking too: a signal can flush the TTY after readiness was observed.
    OpenOptions::new()
        .read(input)
        .write(!input)
        .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
        .open(text)
}

fn open_pair() -> io::Result<(File, File, libc::termios)> {
    let mut master = -1;
    let mut slave = -1;
    // SAFETY: termios is a plain C output structure, fully initialized before use.
    let mut original: libc::termios = unsafe { mem::zeroed() };
    // SAFETY: tcgetattr writes to the initialized local termios structure.
    if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &raw mut original) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: winsize is a plain C output structure, fully initialized before use.
    let mut size: libc::winsize = unsafe { mem::zeroed() };
    // SAFETY: TIOCGWINSZ writes to the initialized local winsize structure.
    if unsafe { libc::ioctl(libc::STDIN_FILENO, libc::TIOCGWINSZ, &raw mut size) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: output pointers refer to initialized storage; no pathname is requested.
    if unsafe {
        libc::openpty(
            &raw mut master,
            &raw mut slave,
            ptr::null_mut(),
            &raw mut original,
            &raw mut size,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful openpty transferred this uniquely owned master descriptor.
    let master = unsafe { File::from_raw_fd(master) };
    // SAFETY: successful openpty transferred this uniquely owned slave descriptor.
    let slave = unsafe { File::from_raw_fd(slave) };
    Ok((master, slave, original))
}

fn raw_mode(original: libc::termios) -> libc::termios {
    let mut mode = original;
    // SAFETY: cfmakeraw updates only the initialized local termios structure.
    unsafe {
        libc::cfmakeraw(&raw mut mode);
    }
    // Keep ISIG: Ctrl-C reaches the CLI, which owns cancellation and forwards it
    // to the private native group. Preserve all other bytes.
    mode.c_lflag |= libc::ISIG;
    mode.c_cc[libc::VQUIT] = libc::_POSIX_VDISABLE;
    mode.c_cc[libc::VSUSP] = libc::_POSIX_VDISABLE;
    #[cfg(target_os = "macos")]
    {
        mode.c_cc[libc::VDSUSP] = libc::_POSIX_VDISABLE;
    }
    mode.c_cc[libc::VMIN] = 1;
    mode.c_cc[libc::VTIME] = 0;
    mode
}
