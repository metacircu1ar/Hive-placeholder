//! Bounded manager subprocesses. The caller supplies its own fork-safe spawn.
#[cfg(target_os = "macos")]
mod group;
use crate::{Result, invalid};
use std::{
    io::Read,
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
pub type Spawn = fn(&mut Command) -> Result<Child>;
#[derive(Debug)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}
pub trait Runner {
    fn run(&self, program: &str, args: &[String]) -> Result<Output>;
}
pub struct SystemRunner(pub Spawn);
impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        capture(self.0, program, args, RUN_TIMEOUT)
    }
}
struct Guard(Child, bool);
impl Guard {
    fn kill_group(&self) -> Result<()> {
        // Private group; leader is retained unreaped until after signalling.
        match rustix::process::kill_process_group(
            rustix::process::Pid::from_child(&self.0),
            rustix::process::Signal::KILL,
        ) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
            #[cfg(target_os = "macos")]
            Err(rustix::io::Errno::PERM) if group::exited_alone(&self.0) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if !self.1 {
            let cleanup = self.kill_group().and_then(|()| self.0.wait());
            if let Err(error) = cleanup {
                tracing::error!(name: "setup.child_cleanup_failed",
                     error = &error as &(dyn std::error::Error + 'static),
                     decision = "preserve_primary_failure");
            }
        }
    }
}
fn nonblocking(fd: &impl std::os::fd::AsFd) -> Result<()> {
    let flags = rustix::fs::fcntl_getfl(fd)?;
    rustix::fs::fcntl_setfl(fd, flags | rustix::fs::OFlags::NONBLOCK)?;
    Ok(())
}
fn drain(input: &mut impl Read, data: &mut Vec<u8>) -> Result<()> {
    loop {
        let mut buf = [0; 4096];
        match input.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                if data.len() + n > 65536 {
                    return Err(invalid("Manager output exceeds 64 KiB."));
                }
                data.extend_from_slice(&buf[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => (),
            Err(e) => return Err(e),
        }
    }
}
pub fn capture(spawn: Spawn, program: &str, args: &[String], timeout: Duration) -> Result<Output> {
    capture_inner(spawn, program, args, Some(timeout), &|| false)
}
/// Long native operations own their transport deadlines. Show progress directly
/// on stderr and bound only the structured stdout, while retaining group cleanup.
pub fn progress(
    spawn: Spawn,
    program: &str,
    args: &[String],
    cancelled: &dyn Fn() -> bool,
) -> Result<Output> {
    capture_inner(spawn, program, args, None, cancelled)
}
fn capture_inner(
    spawn: Spawn,
    program: &str,
    args: &[String],
    timeout: Option<Duration>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Output> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(if timeout.is_some() {
            Stdio::piped()
        } else {
            Stdio::inherit()
        });
    let mut child = Guard(spawn(&mut cmd)?, false);
    let mut stdout = child
        .0
        .stdout
        .take()
        .ok_or_else(|| invalid("Missing stdout pipe."))?;
    let mut stderr = child.0.stderr.take();
    nonblocking(&stdout)?;
    if let Some(stderr) = &stderr {
        nonblocking(stderr)?;
    }
    let mut out = Vec::new();
    let mut err = Vec::new();
    let deadline = timeout.map(|t| Instant::now() + t);
    loop {
        drain(&mut stdout, &mut out)?;
        if let Some(stderr) = &mut stderr {
            drain(stderr, &mut err)?;
        }
        if cancelled() {
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        let exited = match rustix::process::waitid(
            rustix::process::WaitId::Pid(rustix::process::Pid::from_child(&child.0)),
            rustix::process::WaitIdOptions::EXITED
                | rustix::process::WaitIdOptions::NOHANG
                | rustix::process::WaitIdOptions::NOWAIT,
        ) {
            Ok(status) => status.is_some(),
            Err(rustix::io::Errno::INTR) => false,
            Err(e) => return Err(e.into()),
        };
        if exited {
            child.kill_group()?;
            drain(&mut stdout, &mut out)?;
            if let Some(stderr) = &mut stderr {
                drain(stderr, &mut err)?;
            }
            let status = child.0.wait()?;
            child.1 = true;
            return Ok(Output {
                code: status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&err).into_owned(),
                stdout: String::from_utf8(out).map_err(|cause| {
                    crate::context(
                        "Manager returned non-UTF-8 output",
                        std::io::Error::new(std::io::ErrorKind::InvalidData, cause),
                    )
                })?,
            });
        }
        std::thread::sleep(CAPTURE_POLL);
    }
}

const RUN_TIMEOUT: std::time::Duration = Duration::from_secs(30);

const CAPTURE_POLL: std::time::Duration = Duration::from_millis(10);
