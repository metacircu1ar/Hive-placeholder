//! Serialize process launches and wrap cancellation signals and unreaped exit observation.
use super::Signal;
use rustix::{
    io::Errno,
    process::{self as native_process, Pid, WaitId, WaitIdOptions},
};
use std::{
    io, mem,
    os::{
        fd::{FromRawFd, RawFd},
        unix::{net::UnixStream, process::CommandExt},
    },
    process::{Child, Command, Output, Stdio},
    ptr,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicUsize, Ordering},
    },
};

static FORK: Mutex<()> = Mutex::new(());
pub(crate) fn fork_guard() -> io::Result<MutexGuard<'static, ()>> {
    // PoisonError owns the mutex guard; retaining it would extend the launch lock.
    FORK.lock()
        .map_err(|_| io::Error::other("Process launch lock poisoned"))
}
/// The launch lock, if no launch holds it now.
#[cfg(target_os = "linux")]
pub(crate) fn try_fork_guard() -> Option<MutexGuard<'static, ()>> {
    FORK.try_lock().ok()
}
pub(crate) fn spawn(command: &mut Command) -> io::Result<Child> {
    let _fork = fork_guard()?;
    let child = command.spawn()?;
    super::register_child(child.id());
    Ok(child)
}
pub(crate) fn output(command: &mut Command) -> io::Result<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    spawn(command)?.wait_with_output()
}

pub(crate) fn install_cancellation(count: &Arc<AtomicUsize>) -> io::Result<()> {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    for signal in [SIGINT, SIGTERM, SIGHUP] {
        let count = count.clone();
        // SAFETY: The callback only performs a lock-free atomic increment. Its
        // captured Arc keeps the counter alive for every registered invocation.
        unsafe {
            signal_hook::low_level::register(signal, move || {
                if signal == SIGHUP {
                    // Terminal EOF may already have registered this same hangup.
                    let _ = count.compare_exchange(0, 1, Ordering::Relaxed, Ordering::Relaxed);
                } else {
                    count.fetch_add(1, Ordering::Relaxed);
                }
            })
        }?;
    }
    Ok(())
}

/// The caller must still own the unreaped child, never a PID from saved evidence.
pub(crate) fn signal_child(child: &Child, signal: Signal) -> io::Result<()> {
    native_process::kill_process(Pid::from_child(child), signal).map_err(Into::into)
}

pub(crate) fn has_exited(child: &Child) -> io::Result<bool> {
    // NOWAIT reserves the leader's PID until the caller finishes group cleanup.
    let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
    loop {
        match native_process::waitid(WaitId::Pid(Pid::from_child(child)), options) {
            Ok(status) => return Ok(status.is_some()),
            Err(Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

pub(crate) fn new_session() -> io::Result<()> {
    native_process::setsid().map(|_| ()).map_err(Into::into)
}

pub(crate) fn no_such_process(error: &io::Error) -> bool {
    error.raw_os_error() == Some(Errno::SRCH.raw_os_error())
}

/// Ignored SIGCHLD survives exec and would silently discard owned child status.
pub(crate) fn reset_sigchld() -> io::Result<()> {
    // SAFETY: sigaction is a plain output/input C structure, fully initialized.
    let mut action: libc::sigaction = unsafe { mem::zeroed() };
    action.sa_sigaction = libc::SIG_DFL;
    // SAFETY: the mask and action pointers refer to initialized local structures.
    if unsafe { libc::sigemptyset(&raw mut action.sa_mask) } != 0
        // SAFETY: the action is initialized and no previous action is requested.
        || unsafe { libc::sigaction(libc::SIGCHLD, &raw const action, ptr::null_mut()) } != 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Take the gate handed to the dedicated launcher executable by its parent.
pub(crate) fn take_launcher_gate(fd: RawFd) -> UnixStream {
    // SAFETY: the dedicated shim is the only caller and takes sole ownership of
    // its inherited parent-supplied gate before any other Rust owner exists.
    unsafe { UnixStream::from_raw_fd(fd) }
}

pub(crate) fn configure_launcher(command: &mut Command, controlling: bool, fds: Vec<RawFd>) {
    // SAFETY: only async-signal-safe descriptor/group setup runs before exec.
    // The blocking gate is read by the already-executed trusted shim.
    unsafe {
        command.pre_exec(move || {
            if controlling {
                super::new_session()?;
                super::terminal::control_stdin()?;
            } else {
                super::own_process_group()?;
            }
            for fd in &fds {
                super::inherit_retained_fd(*fd)?;
            }
            super::reset_sigchld()?;
            Ok(())
        });
    }
}

pub(crate) fn configure_process_group(command: &mut Command) {
    // SAFETY: only async-signal-safe setpgid and errno conversion occur after fork.
    unsafe {
        command.pre_exec(super::own_process_group);
    }
}
