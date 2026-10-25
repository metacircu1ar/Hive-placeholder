//! Guarded shell helper protocol and command execution.
// Check cancellation while waiting for the owned process group to exit.
use super::diagnostics::{Diagnostics, Phase};
use super::{Request, SOCKET_ENV, TOKEN_PREFIX};
use crate::{
    error::{Error, Result},
    os, process,
};
#[cfg(target_os = "linux")]
use std::path::Path;
use std::{
    env,
    io::{self, BufRead, BufReader, Read, Write},
    iter,
    os::{
        fd::AsRawFd,
        unix::{net::UnixStream, process::CommandExt},
    },
    process::{Child, Command, Stdio, id as process_id},
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;
const POLL_INTERVAL: Duration = Duration::from_millis(10);
// Allow descendants to disappear before declaring cleanup uncertain.
const GROUP_EXIT_GRACE: Duration = Duration::from_secs(10);
// Bound each handshake I/O with the shell guard.
const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);
// Guard replies are short protocol lines; longer input is rejected by parsing.
const REPLY_BYTES: u64 = 64;
// A guard token is a hyphenated UUID embedded in the command.
const TOKEN_BYTES: usize = uuid::fmt::Hyphenated::LENGTH;
// POSIX shell convention when a child was terminated without an exit code.
const INTERRUPTED_EXIT: i32 = 130;

fn failure() -> Error {
    Error::provider_protocol_error(super::SUPERVISION_FAILED)
}

fn bash() -> Command {
    let mut command = Command::new("/bin/bash");
    command.args(["--noprofile", "--norc"]);
    for variable in ["BASH_ENV", "ENV", "SHELLOPTS", "BASHOPTS"] {
        command.env_remove(variable);
    }
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("BASH_FUNC_") {
            command.env_remove(key);
        }
    }
    command
}

pub(super) fn shell() -> Result<i32> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args == ["--version"] {
        println!("GNU bash (Hive supervised entry)");
        return Ok(libc::EXIT_SUCCESS);
    }
    let command = match args.as_slice() {
        [flag, command] if flag == "-c" => command,
        [flag, login, command] if flag == "-c" && login == "-l" => command,
        _ => return Err(failure()),
    };
    // Only the harmless environment probe or an authorized tool envelope may
    // run. In particular, snapshot scripts must never source the user's rc.
    if command != "env" && !command.contains(TOKEN_PREFIX) {
        return Ok(libc::EXIT_FAILURE);
    }
    // exec retains the native process identity and signal forwarding boundary.
    let mut shell = bash();
    shell.args(["-c", command]);
    #[cfg(target_os = "linux")]
    let socket = if command == "env" {
        None
    } else {
        use std::os::fd::AsRawFd;
        let socket = match env::var(super::SOCKET_FD_ENV) {
            Ok(fd) => os::shell::reuse_unconnected(
                fd.parse().map_err(|cause| failure().with_source(cause))?,
            )
            .map_err(Error::state_io_from)?,
            Err(env::VarError::NotPresent) => {
                os::shell::unconnected_socket().map_err(Error::state_io_from)?
            }
            Err(error) => return Err(failure().with_source(error)),
        };
        shell.env(super::SOCKET_FD_ENV, socket.as_raw_fd().to_string());
        Some(socket)
    };
    let error = shell.exec();
    #[cfg(target_os = "linux")]
    drop(socket);
    Err(Error::state_io_from(error))
}

struct Group<'a> {
    child: Option<Child>,
    diagnostics: &'a mut Diagnostics,
}
impl Drop for Group<'_> {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // ECHILD can mean auto-reaping or a lost ownership boundary. Never
            // signal a numeric PGID unless NOWAIT still proves this child ours.
            match os::has_exited(&child) {
                Ok(_) => {
                    if let Err(error) = os::signal_group(&child, os::Signal::KILL) {
                        self.diagnostics.record(Phase::Signal, &error);
                    }
                    if let Err(error) = child.wait() {
                        self.diagnostics.record(Phase::Reap, &error);
                    }
                }
                Err(error) => self.diagnostics.record(Phase::Ownership, &error),
            }
        }
    }
}

fn exchange(stream: &mut BufReader<UnixStream>, message: &Request, expected: &str) -> Result<()> {
    send(stream, message)?;
    let mut reply = String::new();
    stream
        .take(REPLY_BYTES)
        .read_line(&mut reply)
        .map_err(Error::state_io_from)?;
    if reply.trim_end() != expected {
        return Err(failure());
    }
    Ok(())
}

fn send(stream: &mut BufReader<UnixStream>, message: &Request) -> Result<()> {
    let mut bytes = serde_json::to_vec(&super::Envelope::new(message))
        .map_err(|cause| failure().with_source(cause))?;
    bytes.push(b'\n');
    stream
        .get_mut()
        .write_all(&bytes)
        .map_err(Error::state_io_from)
}

pub(super) fn supervise() -> Result<i32> {
    os::reset_sigchld().map_err(Error::state_io_from)?;
    let mut args = env::args().skip(1);
    let command = args.next().ok_or_else(failure)?;
    if command == "--child" {
        return execute_child(&mut args);
    }
    if args.next().is_some() {
        return Err(failure());
    }
    let position = command.find(TOKEN_PREFIX).ok_or_else(failure)? + TOKEN_PREFIX.len();
    let token = command
        .get(position..position + TOKEN_BYTES)
        .ok_or_else(failure)?;
    Uuid::parse_str(token).map_err(|cause| failure().with_source(cause))?;
    let stream = connect_guard()?;
    let mut stream = BufReader::new(stream);
    let cancellation = process::Cancellation::install()?;
    let socket_fd = stream.get_ref().as_raw_fd();
    exchange(
        &mut stream,
        &Request::Register {
            token: token.into(),
            pid: process_id(),
            socket_fd,
        },
        "ready",
    )?;
    let mut diagnostics = Diagnostics::default();
    match run_child(&command, &mut stream, &cancellation, &mut diagnostics) {
        Ok(status) => {
            exchange(&mut stream, &Request::Finished { diagnostics }, "clean")?;
            Ok(status)
        }
        Err(error) => {
            diagnostics.record(Phase::Operation, &error);
            // If the owned connection is gone, the daemon observes its loss and
            // retains uncertainty. Never retry this accepted shell operation.
            if send(&mut stream, &Request::Failed { diagnostics }).is_err() {
                eprintln!("Hive shell: diagnostic delivery failed; cleanup requires verification.");
            }
            Err(error)
        }
    }
}

fn run_child(
    command: &str,
    stream: &mut BufReader<UnixStream>,
    cancellation: &process::Cancellation,
    diagnostics: &mut Diagnostics,
) -> Result<i32> {
    // Pin a live child until the daemon has attached kernel-derived identity.
    // Fast commands can become invisible zombies to macOS proc_pidinfo.
    let mut shell = supervised_command(command)?;
    let child = os::spawn(&mut shell).map_err(Error::state_io_from)?;
    let pid = child.id();
    let mut group = Group {
        child: Some(child),
        diagnostics,
    };
    exchange(stream, &Request::Spawned { pid }, "attached")?;
    let mut gate = group
        .child
        .as_mut()
        .ok_or_else(failure)?
        .stdin
        .take()
        .ok_or_else(failure)?;
    if cancellation.count() == 0 {
        gate.write_all(&[1]).map_err(Error::state_io_from)?;
    }
    drop(gate);

    wait_cleanup(group, cancellation)
}

fn wait_cleanup(mut group: Group<'_>, cancellation: &process::Cancellation) -> Result<i32> {
    let pid = group.child.as_ref().ok_or_else(failure)?.id();
    let child = group.child.as_ref().ok_or_else(failure)?;
    while !process::has_exited(child)? && cancellation.count() == 0 {
        thread::sleep(POLL_INTERVAL);
    }
    // The unreaped leader pins the PGID through the last destructive signal.
    // macOS may return EPERM for a group containing only its zombie; the later
    // read-only absence check is authoritative in that case.
    let deadline = Instant::now() + GROUP_EXIT_GRACE;
    loop {
        if let Err(error) = os::signal_group(child, os::Signal::KILL) {
            group.diagnostics.record(Phase::Signal, &error);
        }
        if os::shell::group_only_leader(pid).map_err(Error::state_io_from)? {
            break;
        }
        if Instant::now() >= deadline {
            return Err(failure());
        }
        thread::sleep(POLL_INTERVAL);
    }
    let status = group
        .child
        .as_mut()
        .ok_or_else(failure)?
        .wait()
        .map_err(Error::state_io_from)?;
    group.child.take();
    let deadline = Instant::now() + GROUP_EXIT_GRACE;
    while !os::group_absent(pid).map_err(Error::state_io_from)? {
        #[cfg(target_os = "linux")]
        os::shell::reap_adopted_group(pid).map_err(Error::state_io_from)?;
        if Instant::now() >= deadline {
            return Err(failure());
        }
        thread::sleep(POLL_INTERVAL);
    }
    Ok(status.code().unwrap_or(INTERRUPTED_EXIT))
}

fn execute_child(args: &mut iter::Skip<env::Args>) -> Result<i32> {
    let body = args.next().ok_or_else(failure)?;
    if args.next().is_some() {
        return Err(failure());
    }
    let mut gate = [0_u8; 1];
    io::stdin()
        .read_exact(&mut gate)
        .map_err(Error::state_io_from)?;
    if gate != [1] {
        return Err(failure());
    }
    Err(Error::state_io_from(
        bash().args(["-c", &body]).stdin(Stdio::null()).exec(),
    ))
}

fn supervised_command(command: &str) -> Result<Command> {
    let mut shell = Command::new(crate::executable::path()?);
    shell
        .args(["--child", command])
        .env_remove(SOCKET_ENV)
        .stdin(Stdio::piped());
    #[cfg(target_os = "linux")]
    {
        shell.env_remove(super::SOCKET_FD_ENV);
        os::shell::adopt_children().map_err(Error::state_io_from)?;
    }
    os::configure_process_group(&mut shell);
    Ok(shell)
}

fn connect_guard() -> Result<UnixStream> {
    let socket = env::var_os(SOCKET_ENV).ok_or_else(failure)?;
    #[cfg(target_os = "macos")]
    let stream = UnixStream::connect(socket).map_err(Error::state_io_from)?;
    #[cfg(target_os = "linux")]
    let stream = {
        let fd = env::var(super::SOCKET_FD_ENV)
            .map_err(|cause| failure().with_source(cause))?
            .parse()
            .map_err(|cause| failure().with_source(cause))?;
        os::shell::connect_inherited(fd, Path::new(&socket)).map_err(Error::state_io_from)?
    };
    stream
        .set_read_timeout(Some(SOCKET_TIMEOUT))
        .map_err(Error::state_io_from)?;
    stream
        .set_write_timeout(Some(SOCKET_TIMEOUT))
        .map_err(Error::state_io_from)?;
    Ok(stream)
}
