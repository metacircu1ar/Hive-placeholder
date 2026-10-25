//! Per-provider shell admission and cleanup evidence. Serviced by the provider's
//! existing I/O loop; no thread or process is reserved for an idle conversation.
// Bound authenticated calls and open peers independently.
use super::{Request, SOCKET_ENV};
use crate::{
    error::{Error, Result},
    os,
    state::ProcessIdentity,
};
use std::{
    collections::HashMap,
    fs,
    io::{self, ErrorKind, Read, Write},
    mem,
    os::{
        fd::{AsFd, BorrowedFd},
        unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::AtomicU64,
    time::{Duration, Instant},
};
use tempfile::TempDir;
use uuid::Uuid;
const MAX_GUARDED_CALLS: usize = 256;
const MAX_CONNECTIONS: usize = 256;
// Bound accepts per tick and back off after descriptor exhaustion.
const ACCEPT_BATCH: usize = 64;
const ACCEPT_BACKOFF: Duration = Duration::from_millis(250);
// Give a new peer time to complete the guard handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
// An unconfirmed guarded command must not stall turn completion indefinitely.
const QUIESCENCE_TIMEOUT: Duration = Duration::from_secs(3);
// Bound individual reads and the accumulated shell-guard request frame.
const CONNECTION_READ_BYTES: usize = 1024;
const CONNECTION_BYTES: usize = 4 * CONNECTION_READ_BYTES;
static REJECTIONS: AtomicU64 = AtomicU64::new(0);

fn rejected(phase: &'static str, error: Option<&(dyn std::error::Error + 'static)>) {
    if let Some(count) = crate::observability::sample(&REJECTIONS) {
        if let Some(error) = error {
            tracing::debug!(name: "shell.peer_rejected",
                 phase = phase,
                 count = count,
                 error = error,
                 decision = "retain_registered_work");
        } else {
            tracing::debug!(name: "shell.peer_rejected",
                 phase = phase,
                 count = count,
                 decision = "retain_registered_work");
        }
    }
}

fn failure() -> Error {
    Error::provider_protocol_error(super::SUPERVISION_FAILED)
}
struct Registration {
    token: String,
    remote_fd: i32,
}
enum ConnectionPhase {
    AwaitingRegistration,
    Registered(Registration),
    Running {
        registration: Registration,
        child: ProcessIdentity,
    },
    Finished,
}
impl ConnectionPhase {
    fn token(&self) -> Option<&str> {
        match self {
            Self::Registered(r)
            | Self::Running {
                registration: r, ..
            } => Some(&r.token),
            Self::AwaitingRegistration | Self::Finished => None,
        }
    }
    fn child(&self) -> Option<&ProcessIdentity> {
        match self {
            Self::Running { child, .. } => Some(child),
            _ => None,
        }
    }
    fn remote_fd(&self) -> Option<i32> {
        match self {
            Self::Registered(r)
            | Self::Running {
                registration: r, ..
            } => Some(r.remote_fd),
            Self::AwaitingRegistration | Self::Finished => None,
        }
    }
    fn finished(&self) -> bool {
        matches!(self, Self::Finished)
    }
}

struct Connection {
    stream: UnixStream,
    peer: os::shell::Peer,
    identity: ProcessIdentity,
    buffer: Vec<u8>,
    phase: ConnectionPhase,
    deadline: Instant,
    lost_at: Option<Instant>,
    output: Vec<u8>,
}
impl Connection {
    fn attach_child(&mut self, root: &Path, provider: u32, pid: u32) -> Result<()> {
        if self.phase.token().is_none() || self.phase.child().is_some() || self.identity.absent()? {
            return Err(failure());
        }
        self.verify_peer(root, provider, self.phase.remote_fd().ok_or_else(failure)?)?;
        let host = os::shell::child(&self.peer, pid).map_err(Error::state_io_from)?;
        let child = ProcessIdentity::read(host)?.ok_or_else(failure)?;
        let ConnectionPhase::Registered(registration) =
            mem::replace(&mut self.phase, ConnectionPhase::AwaitingRegistration)
        else {
            return Err(failure());
        };
        self.phase = ConnectionPhase::Running {
            registration,
            child,
        };
        if self.identity.absent()? {
            return Err(failure());
        }
        Ok(())
    }
    fn verify_peer(&self, root: &Path, provider: u32, socket_fd: i32) -> Result<()> {
        let peer = os::shell::peer(&self.stream, &root.join("hive-shell"), provider)
            .map_err(Error::state_io_from)?;
        if peer.pid != self.peer.pid || peer.local_pid != self.peer.local_pid {
            return Err(failure());
        }
        os::shell::socket_peer(&self.stream, self.peer.pid, socket_fd).map_err(Error::state_io_from)
    }
    fn flush(&mut self) -> io::Result<bool> {
        while !self.output.is_empty() {
            match self.stream.write(&self.output) {
                Ok(0) => return Err(ErrorKind::WriteZero.into()),
                Ok(count) => {
                    self.output.drain(..count);
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(false),
                Err(error) => return Err(error),
            }
        }
        Ok(true)
    }
}
pub(crate) struct ShellGuard {
    _directory: TempDir,
    root: PathBuf,
    listener: UnixListener,
    accept_after: Option<Instant>,
    expected: HashMap<String, bool>,
    connections: Vec<Connection>,
    provider: u32,
    sealed: bool,
    uncertain: bool,
}
impl ShellGuard {
    pub(crate) fn create(
        cwd: &Path,
        instructions: &str,
        binary: impl FnOnce() -> Result<PathBuf>,
    ) -> Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("hive-shell-")
            .permissions(fs::Permissions::from_mode(os::PRIVATE_DIRECTORY_MODE))
            .tempdir_in("/tmp")
            .map_err(Error::state_io_from)?;
        let (directory, root) = crate::publication::prepare(directory, |directory| {
            let parent = directory.canonicalize().map_err(Error::state_io_from)?;
            if parent.starts_with(cwd) {
                return Err(Error::invalid_request(
                    "Agent workspace includes private shell control files.",
                ));
            }
            let current = binary()?;
            let root = parent.join("control");
            prepare::publish(&root, &current, instructions, &atomic_fs::DefaultBarriers)?;
            Ok(root)
        })?;
        let listener = UnixListener::bind(root.join("socket")).map_err(Error::state_io_from)?;
        listener
            .set_nonblocking(true)
            .map_err(Error::state_io_from)?;
        Ok(Self {
            _directory: directory,
            root,
            listener,
            accept_after: None,
            expected: HashMap::new(),
            connections: Vec::new(),
            provider: 0,
            sealed: false,
            uncertain: false,
        })
    }
    pub(crate) fn directory(&self) -> PathBuf {
        self.root.clone()
    }
    pub(crate) fn protected_directory(&self) -> PathBuf {
        // root is canonical and always the control child of our owned TempDir.
        let mut root = self.root.clone();
        root.pop();
        root
    }
    pub(crate) fn configure(&self, command: &mut Command) {
        let root = self.directory();
        command
            .env("CLAUDE_CODE_SHELL", root.join("hive-bash"))
            .env("CLAUDE_CODE_SHELL_PREFIX", root.join("hive-shell"))
            .env(SOCKET_ENV, root.join("socket"))
            .arg("--append-system-prompt-file")
            .arg(root.join("instructions"));
    }
    pub(crate) fn bind(&mut self, pid: u32) {
        self.provider = pid;
    }
    pub(crate) fn expect(&mut self) -> Result<String> {
        if self.sealed || self.uncertain || self.expected.len() >= MAX_GUARDED_CALLS {
            rejected("admission_closed", None);
            return Err(failure());
        }
        let token = Uuid::new_v4().to_string();
        self.expected.insert(token.clone(), false);
        Ok(token)
    }
    pub(crate) fn sealed(&self) -> bool {
        self.sealed
    }
    pub(crate) fn seal(&mut self) {
        self.sealed = true;
        // No helper may spawn without its registration ACK. Unregistered permits
        // are therefore safe to revoke, even if a hook was cancelled midflight.
        self.expected.retain(|_, registered| *registered);
    }
    pub(crate) fn revoke(&mut self, token: &str) {
        if self.expected.get(token) == Some(&false) {
            self.expected.remove(token);
        }
    }
    pub(crate) fn unregistered(&self, token: &str) -> bool {
        !self.sealed && self.expected.get(token) == Some(&false)
    }
    pub(crate) fn sealed_clean(&self) -> bool {
        self.sealed && self.clean()
    }
    pub(crate) fn clean(&self) -> bool {
        !self.uncertain && self.expected.is_empty()
    }
    pub(crate) fn fds(&self) -> Vec<BorrowedFd<'_>> {
        self.accept_after
            .is_none_or(|deadline| Instant::now() >= deadline)
            .then(|| self.listener.as_fd())
            .into_iter()
            .chain(
                self.connections
                    .iter()
                    .filter(|c| c.lost_at.is_none())
                    .map(|c| c.stream.as_fd()),
            )
            .collect()
    }
    pub(crate) fn wait_limit(&self, timeout: Duration) -> Duration {
        let timeout = self.accept_after.map_or(timeout, |deadline| {
            timeout.min(deadline.saturating_duration_since(Instant::now()))
        });
        if self.connections.is_empty() {
            timeout
        } else {
            timeout.min(ACCEPT_BACKOFF)
        }
    }
    pub(crate) fn service(&mut self) -> Result<()> {
        let result = self.service_inner();
        if let Err(error) = &result {
            if !self.uncertain {
                tracing::warn!(name: "shell.cleanup_uncertain",
                     pid = self.provider,
                     error = error as &(dyn std::error::Error + 'static),
                     decision = "retain_ownership");
            }
            self.uncertain = true;
        }
        result
    }
    fn service_inner(&mut self) -> Result<()> {
        self.accept_connections()?;
        let mut index = 0;
        while index < self.connections.len() {
            match self.advance(index) {
                Ok(true) => {
                    self.connections.swap_remove(index);
                }
                Ok(false) => index += 1,
                Err(error) if self.connections[index].phase.token().is_none() => {
                    rejected("unregistered_protocol", Some(&error));
                    self.connections.swap_remove(index);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
    fn accept_connections(&mut self) -> Result<()> {
        for _ in 0..ACCEPT_BATCH {
            if self
                .accept_after
                .is_some_and(|deadline| Instant::now() < deadline)
            {
                break;
            }
            self.accept_after = None;
            let (stream, _) = match self.listener.accept() {
                Ok(value) => value,
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e)
                    if matches!(e.raw_os_error(), Some(libc::EMFILE | libc::ENFILE))
                        || e.kind() == ErrorKind::ConnectionAborted =>
                {
                    // Drop the readable listener from polling while resources
                    // are exhausted; keep servicing already registered work.
                    self.accept_after = Some(Instant::now() + ACCEPT_BACKOFF);
                    rejected("accept_backoff", Some(&e));
                    break;
                }
                Err(e) => return Err(failure().with_source(e)),
            };
            if self.connections.len() >= MAX_CONNECTIONS {
                rejected("connection_capacity", None);
                continue;
            }
            match self.connection(stream) {
                Ok(connection) => self.connections.push(connection),
                Err(error) => rejected("peer_identity", Some(&error)),
            }
        }
        Ok(())
    }
    fn connection(&self, stream: UnixStream) -> Result<Connection> {
        stream.set_nonblocking(true).map_err(Error::state_io_from)?;
        let peer = os::shell::peer(&stream, &self.root.join("hive-shell"), self.provider)
            .map_err(Error::state_io_from)?;
        let identity = ProcessIdentity::read(peer.pid)?.ok_or_else(failure)?;
        Ok(Connection {
            stream,
            peer,
            identity,
            buffer: Vec::new(),
            phase: ConnectionPhase::AwaitingRegistration,
            deadline: Instant::now() + HANDSHAKE_TIMEOUT,
            lost_at: None,
            output: Vec::new(),
        })
    }
    fn advance(&mut self, index: usize) -> Result<bool> {
        let connection = &mut self.connections[index];
        if connection.lost_at.is_some() {
            return self.lost(index);
        }
        match connection.flush() {
            Ok(true) => {}
            Ok(false) => return Ok(false),
            Err(error) => {
                rejected("reply_flush", Some(&error));
                return self.lost(index);
            }
        }
        if connection.phase.finished() {
            return Ok(true);
        }
        if !connection.buffer.contains(&b'\n') {
            let mut bytes = [0; CONNECTION_READ_BYTES];
            match connection.stream.read(&mut bytes) {
                Ok(0) => return self.lost(index),
                Ok(count) => connection.buffer.extend_from_slice(&bytes[..count]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if connection.identity.absent()? {
                        return self.lost(index);
                    }
                    if connection.phase.child().is_none() && Instant::now() > connection.deadline {
                        rejected("handshake_timeout", None);
                        return Err(failure());
                    }
                    return Ok(false);
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => return Ok(false),
                Err(error) => {
                    rejected("request_read", Some(&error));
                    return self.lost(index);
                }
            }
        }
        if connection.buffer.len() > CONNECTION_BYTES {
            return Err(failure());
        }
        let Some(end) = connection.buffer.iter().position(|b| *b == b'\n') else {
            return Ok(false);
        };
        let bytes = connection.buffer.drain(..=end).collect::<Vec<_>>();
        let mut decoder = serde_json::Deserializer::from_slice(&bytes);
        let message: super::Envelope<Request> =
            crate::runtime::domain::decode::object(&mut decoder)
                .map_err(|cause| failure().with_source(cause))?;
        decoder
            .end()
            .map_err(|cause| failure().with_source(cause))?;
        self.respond(index, message.request)
    }
    fn respond(&mut self, index: usize, message: Request) -> Result<bool> {
        let connection = &mut self.connections[index];
        let response = match message {
            Request::Register {
                token,
                pid,
                socket_fd,
            } => {
                if connection.phase.token().is_none()
                    && (self.sealed || self.expected.get(&token) != Some(&false))
                {
                    // Admission was revoked before ACK; no child was authorized.
                    rejected("revoked_admission", None);
                    return Ok(true);
                }
                if connection.phase.token().is_some()
                    || pid != connection.peer.local_pid
                    || connection.identity.absent()?
                    || self.expected.get(&token) != Some(&false)
                {
                    return Err(failure());
                }
                connection.verify_peer(&self.root, self.provider, socket_fd)?;
                if connection.identity.absent()? {
                    return Err(failure());
                }
                self.expected.insert(token.clone(), true);
                connection.phase = ConnectionPhase::Registered(Registration {
                    token,
                    remote_fd: socket_fd,
                });
                "ready\n"
            }
            Request::Spawned { pid } => {
                connection.attach_child(&self.root, self.provider, pid)?;
                "attached\n"
            }
            Request::Finished { diagnostics } => {
                connection.verify_finished(&self.root, self.provider)?;
                diagnostics.log(connection.peer.pid);
                self.expected
                    .remove(connection.phase.token().ok_or_else(failure)?);
                connection.phase = ConnectionPhase::Finished;
                "clean\n"
            }
            Request::Failed { diagnostics } => {
                if connection.phase.token().is_some() {
                    diagnostics.log(connection.peer.pid);
                    // A diagnostic is not cleanup evidence. Use the same kernel
                    // proof and grace period as an EOF from the helper.
                    return self.lost(index);
                }
                return Err(failure());
            }
        };
        connection.output.extend_from_slice(response.as_bytes());
        match connection.flush() {
            Ok(true) => Ok(connection.phase.finished()),
            Ok(false) => Ok(false),
            Err(error) => {
                rejected("response_flush", Some(&error));
                self.lost(index)
            }
        }
    }
    fn lost(&mut self, index: usize) -> Result<bool> {
        let connection = &mut self.connections[index];
        if connection.phase.finished() || connection.phase.token().is_none() {
            return Ok(true);
        }
        if let Some(child) = connection.phase.child() {
            reap_adopted(&connection.identity)?;
            reap_adopted(child)?;
            os::reap_group(child.pid).map_err(Error::state_io_from)?;
        }
        if let Some(child) = connection.phase.child()
            && connection.identity.absent()?
            && child.absent()?
            && os::group_absent(child.pid).map_err(Error::state_io_from)?
        {
            // Native timeout/tree-kill may remove the helper before its receipt.
            // Kernel evidence still proves cleanup. Never signal a saved PGID.
            self.expected
                .remove(connection.phase.token().ok_or_else(failure)?);
            return Ok(true);
        }
        let since = connection.lost_at.get_or_insert_with(Instant::now);
        if since.elapsed() >= QUIESCENCE_TIMEOUT {
            Err(failure())
        } else {
            Ok(false)
        }
    }
}

/// Reap a helper or command this daemon adopted after its parent ended. A zombie
/// keeps its identity and process group present.
fn reap_adopted(identity: &ProcessIdentity) -> Result<()> {
    if let Some(start) = identity
        .birth
        .strip_prefix("linux:")
        .and_then(|ticks| ticks.parse().ok())
    {
        os::reap_process(identity.pid, start).map_err(Error::state_io_from)?;
    }
    Ok(())
}

impl Connection {
    fn verify_finished(&self, root: &Path, provider: u32) -> Result<()> {
        let child = self.phase.child().ok_or_else(failure)?;
        reap_adopted(&self.identity)?;
        reap_adopted(child)?;
        os::reap_group(child.pid).map_err(Error::state_io_from)?;
        if !child.absent()? || !os::group_absent(child.pid).map_err(Error::state_io_from)? {
            return Err(failure());
        }
        if !self.identity.absent()? {
            self.verify_peer(root, provider, self.phase.remote_fd().ok_or_else(failure)?)?;
        }
        Ok(())
    }
}

mod prepare;
#[cfg(test)]
mod tests;
