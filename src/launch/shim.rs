//! Dispatch the trusted shim and execute native providers after a valid gate grant.
use crate::{
    error::{Error, Result},
    os, process,
};
use std::{
    env,
    ffi::OsStr,
    io::{ErrorKind, Read, Write},
    os::{fd::AsFd, unix::process::CommandExt},
    path::Path,
    process::{Command, exit},
    time::Instant,
};

#[doc(hidden)]
pub(crate) fn dispatch() {
    let mut args = env::args_os();
    if args
        .next()
        .as_deref()
        .and_then(|p| Path::new(p).file_name())
        .is_none_or(|n| n != "hive-launch")
    {
        return;
    }
    let result = execute(args);
    if let Err(error) = result {
        eprintln!("Hive launcher: {error}");
    }
    exit(process::EXEC_FAILED_EXIT)
}

fn execute(mut args: env::ArgsOs) -> Result<()> {
    let first = args.next();
    if first.as_deref() == Some(OsStr::new("--hive-launch-self-check")) {
        exit(libc::EXIT_SUCCESS);
    }
    let fd = first
        .and_then(|s| s.to_str().and_then(|s| s.parse::<i32>().ok()))
        .filter(|n| *n > libc::STDERR_FILENO)
        .ok_or_else(|| {
            Error::spawn_failed("Trusted launcher received an invalid gate descriptor.")
        })?;
    let binary = args
        .next()
        .ok_or_else(|| Error::spawn_failed("Trusted launcher received no provider executable."))?;
    let mut gate = os::take_launcher_gate(fd);
    // macOS rejects SO_RCVTIMEO on an AF_UNIX socket after its peer
    // closes, even when the grant is buffered. Poll then read nonblocking.
    os::nonblocking(&gate).map_err(Error::state_io_from)?;
    let deadline = Instant::now() + super::GATE_TIMEOUT;
    let mut grant = [0; 1];
    loop {
        os::poll_readable(
            [Some(gate.as_fd()), None, None],
            deadline.saturating_duration_since(Instant::now()),
        )
        .map_err(Error::state_io_from)?;
        match gate.read(&mut grant) {
            Ok(1) if grant == *b"G" => break,
            Err(e)
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted)
                    && Instant::now() < deadline => {}
            _ => {
                return Err(Error::spawn_failed(
                    "Native execution was not granted before the launch gate closed or timed out.",
                ));
            }
        }
    }
    os::close_on_exec(gate.as_fd()).map_err(Error::state_io_from)?;
    let error = Command::new(binary).args(args).exec();
    let mut status = Vec::from(*super::STATUS_TAG);
    status.extend_from_slice(&error.raw_os_error().unwrap_or(libc::EIO).to_ne_bytes());
    if let Err(delivery) = gate.write_all(&status) {
        // There is no subscriber in the shim and the parent's diagnostic
        // channel is gone. Keep the exec failure primary and print only a fixed
        // classification/native number, never an executable or argument.
        eprintln!(
            "Hive launcher: status_delivery_failed errno={}",
            delivery.raw_os_error().unwrap_or(0)
        );
    }
    Err(Error::spawn_failed(super::NATIVE_EXEC_FAILED).with_source(error))
}
