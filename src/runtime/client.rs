//! Bounded control-socket requests to the local daemon.
use super::api;
use crate::calls::ObservationError;
use crate::error::{Error, Result};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};
pub(crate) mod envelope;
mod startup;

// How long the control client waits for a daemon response.
const CLIENT_READ_TIMEOUT: Duration = Duration::from_secs(40);
// How long the control client waits to write its request.
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
// Maximum bytes read from a control response before JSON decoding.
const CLIENT_RESPONSE_READ_BYTES: u64 = crate::limits::JOIN_TRANSPORT_BYTES as u64;
pub fn response(result: Result<Value>) -> Value {
    match result {
        Ok(value) => value,
        Err(error) => api::value(api::ErrorResponse { error }),
    }
}
pub(super) fn invalid_request() -> Error {
    Error::invalid_request("Invalid operation or arguments.")
}
pub fn socket_path(root: &Path) -> PathBuf {
    root.join("runtime/control.sock")
}
pub fn client(root: &Path, id: &str, args: impl Serialize) -> Result<Value> {
    Client::new(root).request(id, args)
}

fn unavailable(cause: std::io::Error) -> Error {
    Error::daemon_unavailable("Start the Hive daemon for this state directory first.")
        .with_source(cause)
}

#[derive(Serialize)]
struct ClientRequest<'a, T> {
    protocol: u32,
    id: &'a str,
    args: T,
}
fn request(id: &str, args: impl Serialize) -> Value {
    api::value(ClientRequest {
        protocol: crate::compatibility::CONTROL_PROTOCOL,
        id,
        args,
    })
}

pub(crate) struct Client<'a> {
    root: &'a Path,
}
impl<'a> Client<'a> {
    pub fn new(root: &'a Path) -> Self {
        Self { root }
    }
    /// Synchronous commands keep per-I/O socket timeouts and leave success
    /// validation to their operation-specific response reader.
    pub fn request(&self, id: &str, args: impl Serialize) -> Result<Value> {
        let (line, value) = self.receive(id, args)?;
        envelope::validate(&line, value)
    }

    /// GC alone retries a lost/malformed JSON reply with the same durable key.
    /// A complete JSON envelope that violates the protocol is never retried.
    pub(crate) fn request_retrying_transport(
        &self,
        id: &str,
        args: &api::Request,
    ) -> Result<Value> {
        let (line, value) = self.receive(id, args).or_else(|error| {
            tracing::warn!(name: "gc.transport_retry",
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "reuse_durable_request_id");
            self.receive(id, args)
        })?;
        envelope::validate(&line, value)
    }

    fn receive(&self, id: &str, args: impl Serialize) -> Result<(String, Value)> {
        let mut stream = UnixStream::connect(socket_path(self.root)).map_err(unavailable)?;
        stream
            .set_read_timeout(Some(CLIENT_READ_TIMEOUT))
            .map_err(Error::state_io_from)?;
        stream
            .set_write_timeout(Some(CLIENT_WRITE_TIMEOUT))
            .map_err(Error::state_io_from)?;
        writeln!(stream, "{}", request(id, args)).map_err(Error::state_io_from)?;
        let mut data = String::new();
        BufReader::new(stream)
            .take(CLIENT_RESPONSE_READ_BYTES)
            .read_line(&mut data)
            .map_err(Error::state_io_from)?;
        let value = envelope::json(&data)?;
        Ok((data, value))
    }
    /// Observations keep one cancellable deadline across connect, write, read,
    /// and typed decode; a blocking adapter would outlive cancellation.
    pub async fn observe<T: DeserializeOwned>(
        &self,
        id: &str,
        args: &api::Request,
        deadline: tokio::time::Instant,
    ) -> std::result::Result<T, ObservationError> {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
        let operation = async {
            let mut stream = tokio::net::UnixStream::connect(socket_path(self.root))
                .await
                .map_err(unavailable)?;
            #[expect(
                clippy::expect_used,
                reason = "A JSON Value has no fallible serializer"
            )]
            let mut request = serde_json::to_vec(&request(id, args)).expect("serializable request");
            request.push(b'\n');
            stream
                .write_all(&request)
                .await
                .map_err(Error::state_io_from)?;
            let mut line = String::new();
            tokio::io::BufReader::new(stream.take(CLIENT_RESPONSE_READ_BYTES))
                .read_line(&mut line)
                .await
                .map_err(Error::state_io_from)?;
            decode(&line)
        };
        match tokio::time::timeout_at(deadline, operation).await {
            Ok(Ok(Reply::Success(value))) => Ok(value),
            Ok(Ok(Reply::Error { error })) => Err(error),
            Ok(Err(error)) => Err(error.into()),
            // Timeout is an observation outcome; the elapsed marker carries no cause.
            Err(_) => Err(ObservationError::deadline()),
        }
    }
}

pub(crate) enum Reply<T> {
    Error { error: ObservationError },
    Success(T),
}

pub(crate) fn decode<T: DeserializeOwned>(line: &str) -> Result<Reply<T>> {
    let raw = envelope::parse(line)?;
    if let Some(error) = envelope::error(&raw, envelope::Policy::Observation)? {
        return Ok(Reply::Error { error: error.error });
    }
    serde_json::from_value(raw)
        .map(Reply::Success)
        .map_err(|cause| envelope::invalid().with_source(cause))
}

#[cfg(test)]
#[test]
fn compatibility_control_request_contract() {
    crate::compatibility::contract(
        "control-envelope.json",
        &request("request-1", api::Request::Ping),
    );
}

/// The newest page, the page after a cursor, and the page before one.
#[cfg(test)]
fn attach_requests() -> Vec<Value> {
    let attach = |id, args: api::AttachArgs| {
        request(
            id,
            api::Request::Attach(api::AttachArgs {
                target: "AGENT_ID".to_owned().into(),
                ..args
            }),
        )
    };
    vec![
        attach(
            "attach-1",
            api::AttachArgs {
                limit: 100.into(),
                ..api::AttachArgs::default()
            },
        ),
        attach(
            "attach-after-1",
            api::AttachArgs {
                after: 40.into(),
                ..api::AttachArgs::default()
            },
        ),
        attach(
            "attach-before-1",
            api::AttachArgs {
                before: 40.into(),
                limit: 50.into(),
                ..api::AttachArgs::default()
            },
        ),
    ]
}

/// Two providers without a default, a group and roles resolving each provider.
#[cfg(test)]
fn roles_config() -> crate::config::Config {
    crate::config::Config::parse(concat!(
        "auth_profiles:\n",
        "  main: {provider: codex, method: subscription}\n",
        "  claude-main: {provider: claude, method: subscription}\n",
        "auth_groups: {plans: {mode: ordered, members: [main]}}\n",
        "providers: {codex: {auth: main, model: terra}, claude: {auth: claude-main, model: sonnet}}\n",

        "roles: {reviewer: {auth: claude-main}, worker: {provider: codex}}\n",
    ))
    .unwrap()
}

#[cfg(test)]
#[test]
fn compatibility_live_control_contract() {
    use crate::runtime::live::{Cursor, TreeVersion};
    let cursor = Cursor {
        epoch: 7,
        generation: 1,
        seq: 3,
        durable: 2,
    };
    let version = TreeVersion {
        epoch: 7,
        revision: 4,
    };
    let mut requests = attach_requests();
    requests.extend([
        request(
            "live-1",
            api::Request::Live(api::LiveArgs {
                target: "AGENT_ID".to_owned().into(),
                after: cursor.into(),
                wait_ms: 25_000.into(),
                limit: 512.into(),
            }),
        ),
        request(
            "tree-1",
            api::Request::Tree(api::TreeArgs {
                root: "AGENT_ID".to_owned().into(),
                after: version.into(),
                wait_ms: 25_000.into(),
                ..api::TreeArgs::default()
            }),
        ),
        request(
            "tree-all-1",
            api::Request::Tree(api::TreeArgs {
                all: true.into(),
                ..api::TreeArgs::default()
            }),
        ),
        request("roles-1", api::Request::Roles),
    ]);
    let config = roles_config();
    let replies = serde_json::json!({
        "attach": api::AttachReply {
            live: cursor,
            snapshot: serde_json::json!({"agent": {},
                 "events": {},
                 "queued": 0,
                 "mail": 0,
                 "children": 0}),

        },
        "live": crate::runtime::live::Page {
            cursor: Some(cursor),
            items: Vec::new(),
            resync: false,
            durable_changed: true,
        },
        "attach_history": api::HistoryPage::<Vec<serde_json::Value>> {
            events: Vec::new(),
            older: Some(12),
            pruned_through: 0,
            history_expired: false,
        },
        "tree": api::TreeReply {
            version,
            tree: serde_json::json!({"agents": [], "truncated": false}),
        },
        "roles": api::RolesResponse::new(&config),
        "watch_capacity": api::ErrorResponse {
            error: crate::error::Error::watch_capacity(
                "Too many viewers are waiting; retry after a short delay.",
            ),
        },
    });
    crate::compatibility::contract(
        "control-live.json",
        &serde_json::json!({"requests": requests, "replies": replies}),
    );
}
