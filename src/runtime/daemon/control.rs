//! Independently scheduled, bounded control connections with owned shutdown.
use super::super::{
    Shared,
    api::{self, Request, RequestEnvelope},
    hub, invalid_request, provider,
    reader::Reader,
    response,
};
use crate::error::{Error, Result};
use serde::Deserialize;
use serde_json::Value;
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::UnixListener,
    sync::oneshot,
    task::{JoinHandle, JoinSet},
};

// Maximum concurrent control connections admitted by the daemon.
const CONTROL_CLIENT_LIMIT: usize = 64;
// Delay after a listener accept error before retrying.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);
// How long a control connection may take to send its request line.
const SERVER_READ_TIMEOUT: Duration = Duration::from_secs(5);
// Maximum request-line bytes read before JSON decoding.
const SERVER_REQUEST_READ_BYTES: u64 = 1024 * 1024;
// Maximum control request-ID length accepted for replay tracking.
// How long a control connection may take to accept its response.
const SERVER_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) struct Started {
    pub stop: oneshot::Sender<()>,
    pub task: JoinHandle<Result<()>>,
}

pub(super) fn start(
    listener: UnixListener,
    shared: Shared,
    reader: Reader,
    pool: provider::Pool,
    shutdown: Arc<AtomicBool>,
) -> Started {
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut clients = JoinSet::new();
        let result = accept_loop(&listener, &shared, &reader, &pool, &mut clients, stopped).await;
        // Viewers' long-polls end now, so joining their handlers cannot delay shutdown.
        shared.wake.live.stop();
        // Close the kernel backlog before waiting for accepted clients. Keep their
        // Shared/pool ownership until every handler has finished its response.
        drop(listener);
        finish(result, &mut clients, &shutdown, &shared.wake).await
    });
    Started { stop, task }
}

async fn finish(
    result: Result<()>,
    clients: &mut JoinSet<()>,
    shutdown: &AtomicBool,
    wake: &hub::Wake,
) -> Result<()> {
    if let Err(error) = &result {
        wake.failed(error);
        shutdown.store(true, Ordering::Relaxed);
        wake.all();
    }
    while let Some(joined) = clients.join_next().await {
        handler_join(joined);
    }
    result
}

async fn accept_loop(
    listener: &UnixListener,
    shared: &Shared,
    reader: &Reader,
    pool: &provider::Pool,
    clients: &mut JoinSet<()>,
    mut stopped: oneshot::Receiver<()>,
) -> Result<()> {
    let mut acceptor = ClientAcceptor {
        shared,
        reader,
        pool,
        clients,
    };
    loop {
        let capacity = has_client_capacity(acceptor.clients);
        tokio::select! {
            biased;
            _ = &mut stopped => return Ok(()),
            accepted = listener.accept(), if capacity => {
                if acceptor.accepted(accepted, &mut stopped).await? {
                    return Ok(());
                }
            },
            joined = acceptor.clients.join_next(), if !acceptor.clients.is_empty() => {
                if let Some(joined) = joined { handler_join(joined); }
            }
        }
    }
}

struct ClientAcceptor<'a> {
    shared: &'a Shared,
    reader: &'a Reader,
    pool: &'a provider::Pool,
    clients: &'a mut JoinSet<()>,
}
impl ClientAcceptor<'_> {
    async fn accepted(
        &mut self,
        accepted: std::io::Result<(tokio::net::UnixStream, tokio::net::unix::SocketAddr)>,
        stopped: &mut oneshot::Receiver<()>,
    ) -> Result<bool> {
        match accepted {
            Ok((stream, _)) => {
                let shared = self.shared.clone();
                let pool = self.pool.clone();
                let reader = self.reader.clone();
                self.clients.spawn(async move {
                    serve_client(shared, reader, pool, stream).await;
                });
                Ok(false)
            }
            Err(error) if retryable(&error) => {
                tracing::warn!(name: "control.accept_backoff",
                     error = &error as &(dyn std::error::Error + 'static),
                     decision = "retry_after_backoff");
                tokio::select! {
                    _ = stopped => Ok(true),
                    () = tokio::time::sleep(ACCEPT_BACKOFF) => Ok(false),
                }
            }
            Err(error) => Err(Error::state_io_from(error)),
        }
    }
}

fn retryable(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::Interrupted | std::io::ErrorKind::ConnectionAborted
    ) || crate::os::descriptors_exhausted(error)
}

pub(super) struct Supervision {
    pub supervisor: Result<()>,
    pub control: Option<Result<()>>,
}

/// A control failure requests shutdown but must never cancel a partially awaited
/// admission or claim flush. Consume the control handle exactly once.
pub(super) async fn supervise(
    supervisor: impl Future<Output = Result<()>>,
    control: &mut JoinHandle<Result<()>>,
    shutdown: &AtomicBool,
    uncertain: &AtomicBool,
    wake: &hub::Wake,
) -> Supervision {
    tokio::pin!(supervisor);
    tokio::select! {
        result = &mut supervisor => {
            if let Err(error) = &result { wake.failed(error); }
            Supervision { supervisor: result, control: None }
        },
        joined = control => control_finished(supervisor, joined, shutdown, uncertain, wake).await,
    }
}

async fn control_finished(
    supervisor: impl Future<Output = Result<()>>,
    joined: std::result::Result<Result<()>, tokio::task::JoinError>,
    shutdown: &AtomicBool,
    uncertain: &AtomicBool,
    wake: &hub::Wake,
) -> Supervision {
    let result = outcome(joined, uncertain);
    if let Err(error) = &result {
        wake.failed(error);
    }
    shutdown.store(true, Ordering::Relaxed);
    wake.all();
    let supervisor = supervisor.await;
    if let Err(error) = &supervisor {
        wake.failed(error);
    }
    Supervision {
        supervisor,
        control: Some(result),
    }
}

pub(super) async fn join(
    task: JoinHandle<Result<()>>,
    consumed: Option<Result<()>>,
    uncertain: &AtomicBool,
) -> Result<()> {
    match consumed {
        Some(result) => result,
        None => outcome(task.await, uncertain),
    }
}

fn outcome(
    joined: std::result::Result<Result<()>, tokio::task::JoinError>,
    uncertain: &AtomicBool,
) -> Result<()> {
    joined.unwrap_or_else(|error| {
        // Unwinding the owner's JoinSet aborts handlers; their joins were not
        // confirmed. Preserve ownership evidence even after best-effort cleanup.
        uncertain.store(true, Ordering::Relaxed);
        Err(Error::daemon_stopping("Control task did not finish normally.").with_source(error))
    })
}

fn has_client_capacity(clients: &mut tokio::task::JoinSet<()>) -> bool {
    // JoinSet retains completed tasks until collected. Only live requests consume
    // slots, even if acceptance wins repeatedly over the completion branch.
    while let Some(joined) = clients.try_join_next() {
        handler_join(joined);
    }
    clients.len() < CONTROL_CLIENT_LIMIT
}

fn handler_join(joined: std::result::Result<(), tokio::task::JoinError>) {
    if let Err(error) = joined {
        let code = if error.is_panic() {
            "panicked"
        } else {
            "cancelled"
        };
        let task_id = error.id().to_string();
        tracing::warn!(name: "control.handler_failed",
             task_id = task_id,
             thread = "control_handler",
             code = code,
             error = &error as &(dyn std::error::Error + 'static),
             decision = "retain_siblings");
    }
}

async fn serve_client(
    shared: Shared,
    reader: Reader,
    pool: provider::Pool,
    stream: tokio::net::UnixStream,
) {
    use tokio::io::AsyncWriteExt;
    let (read, mut write) = stream.into_split();
    let mut purge_request_id = None;
    let result = ClientRequest {
        shared: &shared,
        reader: &reader,
        pool: &pool,
    }
    .read(read, &mut purge_request_id)
    .await;
    let response = api::ControlReply {
        request_id: purge_request_id,
        ..Default::default()
    }
    .decorate(response(result));
    let data = format!("{response}\n");
    match tokio::time::timeout(SERVER_WRITE_TIMEOUT, write.write_all(data.as_bytes())).await {
        Ok(Ok(())) => (),
        Ok(Err(error)) => crate::observability::lost_reply(
            crate::observability::ReplyOwner::Control,
            Some(&error),
        ),
        Err(error) => crate::observability::lost_reply(
            crate::observability::ReplyOwner::Control,
            Some(&error),
        ),
    }
}

struct IncomingRequest {
    id: String,
    args: RequestEnvelope,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlInput {
    protocol: u32,
    #[serde(default)]
    id: api::Input<String>,
    #[serde(default)]
    args: Value,
}

async fn read_request(read: tokio::net::unix::OwnedReadHalf) -> Result<IncomingRequest> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    let mut line = String::new();
    tokio::time::timeout(
        SERVER_READ_TIMEOUT,
        BufReader::new(read.take(SERVER_REQUEST_READ_BYTES)).read_line(&mut line),
    )
    .await
    .map_err(|cause| invalid_request().with_source(cause))?
    .map_err(Error::state_io_from)?;
    let request_data: Value =
        serde_json::from_str(&line).map_err(|cause| invalid_request().with_source(cause))?;
    // The current outer protocol is strict; operation argument handling retains
    // its existing exact raw fingerprint and actionable validation semantics.
    if !request_data.is_object() {
        return Err(invalid_request());
    }
    let request_data: ControlInput =
        serde_json::from_str(&line).map_err(|cause| invalid_request().with_source(cause))?;
    if !crate::compatibility::CONTROL_RANGE.contains(request_data.protocol) {
        return Err(Error::IncompatibleProtocol {
            message: format!(
                "Unsupported control protocol. Hive {} supports {} through {}; update the \
                CLI or daemon to a compatible build.",
                env!("CARGO_PKG_VERSION"),
                crate::compatibility::CONTROL_RANGE.minimum,
                crate::compatibility::CONTROL_RANGE.maximum
            )
            .into(),
            source: None,
        });
    }
    let identity_error = || {
        api::InputContext::Control.error(
            hive_input::Code::InvalidRequestId,
            "/id",
            "a request ID string of 1..128 UTF-8 bytes without control characters",
        )
    };
    let id = request_data
        .id
        .optional(api::InputContext::Control, "/id", "a request ID string")
        .map_err(|cause| identity_error().with_source(cause))?
        .ok_or_else(identity_error)?;
    hive_input::validate::request_id(id).map_err(|cause| identity_error().with_source(cause))?;
    let id = id.clone();
    let args = RequestEnvelope::new(request_data.args);
    Ok(IncomingRequest { id, args })
}
struct ClientRequest<'a> {
    shared: &'a Shared,
    reader: &'a Reader,
    pool: &'a provider::Pool,
}
impl ClientRequest<'_> {
    async fn read(
        &self,
        read: tokio::net::unix::OwnedReadHalf,
        purge_request_id: &mut Option<String>,
    ) -> Result<Value> {
        let IncomingRequest { id, args } = read_request(read).await?;
        self.execute(id, args, purge_request_id).await
    }
    async fn execute(
        &self,
        id: String,
        args: RequestEnvelope,
        purge_request_id: &mut Option<String>,
    ) -> Result<Value> {
        let Self {
            shared,
            reader,
            pool,
        } = self;
        match &args.request {
            Request::Live(live) => return live_items(shared, live).await,
            Request::Tree(_) => return tree(shared, reader, args).await,
            Request::Attach(_) => return attach(shared, reader, args).await,
            Request::Usage(args) => return usage(pool, reader, args),
            _ => {}
        }
        let gc = matches!(&args.request, Request::Gc(g) if g.dry_run.copied() != Some(true));
        let purge = matches!(&args.request, Request::Purge(p) if p.dry_run.copied() != Some(true));
        if purge || gc {
            *purge_request_id = Some(id.clone());
        }
        let status = matches!(args.request, Request::Ping);
        let crate::runtime::reader::Reply { value } = if args.request.is_read_only() {
            reader.request(&hub::Caller::Human, args).await?
        } else {
            shared
                .dispatch(hub::Caller::Human, id, args)
                .await
                .map(|value| crate::runtime::reader::Reply { value })?
        };
        let cleanup_pending = if gc {
            Some(finish_purge_batch(shared, reader).await.map_err(|error| {
                Error::purge_cleanup_pending(concat!(
                    "Agent records were purged but output cleanup is incomplete. ",
                    "Retry with the returned request_id."
                ))
                .with_source(error)
            })?)
        } else {
            None
        };
        if purge {
            finish_purge(shared, reader).await.map_err(|error| {
                Error::purge_cleanup_pending(concat!(
                    "Agent records were purged but output cleanup is incomplete. ",
                    "Retry with the returned request_id."
                ))
                .with_source(error)
            })?;
        }
        let runtime_resources = status.then(|| pool.snapshot());
        Ok(api::ControlReply {
            request_id: None,
            cleanup_pending,
            runtime_resources,
        }
        .decorate(value))
    }
}

/// A long-poll wait: 0..=25000 ms, defaulting to the longest wait.
fn wait(input: &api::Input<u64>, context: api::InputContext) -> Result<Duration> {
    const EXPECTED: &str = "an integer from 0 through 25000 milliseconds";
    let maximum = u64::try_from(crate::runtime::live::MAX_WAIT.as_millis()).unwrap_or(u64::MAX);
    match input.optional(context, "/wait_ms", EXPECTED)?.copied() {
        None => Ok(crate::runtime::live::MAX_WAIT),
        Some(ms) if ms <= maximum => Ok(Duration::from_millis(ms)),
        Some(_) => Err(context.error(hive_input::Code::InvalidValue, "/wait_ms", EXPECTED)),
    }
}

/// Wait outside the reader and writer, holding one of the long-poll permits.
async fn live_items(shared: &Shared, args: &api::LiveArgs) -> Result<Value> {
    use api::InputContext as Op;
    const CURSOR: &str = "a cursor returned by attach or live";
    const LIMIT: &str = "an integer from 1 through 512";
    let hub = &shared.wake.live;
    let target = args.target.required(Op::Live, "/target")?;
    let after = *args
        .after
        .optional(Op::Live, "/after", CURSOR)?
        .ok_or_else(|| Op::Live.error(hive_input::Code::MissingArgument, "/after", CURSOR))?;
    let wait = wait(&args.wait_ms, Op::Live)?;
    let limit = match args.limit.optional(Op::Live, "/limit", LIMIT)?.copied() {
        None => crate::runtime::live::MAX_PAGE_ITEMS,
        Some(limit) => usize::try_from(limit)
            .ok()
            .filter(|limit| (1..=crate::runtime::live::MAX_PAGE_ITEMS).contains(limit))
            .ok_or_else(|| Op::Live.error(hive_input::Code::InvalidValue, "/limit", LIMIT))?,
    };
    let _permit = hub.admit_if(!wait.is_zero())?;
    hub.next(target, after, wait, limit).await.map(api::value)
}

/// The quota usage each login's provider last reported, from memory. A named
/// login must be configured.
fn usage(pool: &provider::Pool, reader: &Reader, args: &api::UsageArgs) -> Result<Value> {
    let auth = args
        .auth
        .optional(api::InputContext::Usage, "/auth", "a login profile name")?;
    if let Some(auth) = auth {
        reader.config().profile(auth)?;
    }
    let reports = pool.usage.reports(auth.map(String::as_str));
    Ok(api::value(api::UsageReply {
        usage: api::value(reports),
    }))
}

/// Wait for a tree change, then read rows from one reader snapshot. The version
/// is taken first, so a change during the snapshot wakes the next poll at once.
async fn tree(shared: &Shared, reader: &Reader, args: RequestEnvelope) -> Result<Value> {
    use api::InputContext as Op;
    let Request::Tree(tree) = &args.request else {
        return Err(invalid_request());
    };
    let after = tree
        .after
        .optional(Op::Tree, "/after", "a version returned by tree")?
        .copied();
    let wait = wait(&tree.wait_ms, Op::Tree)?;
    let version = {
        let _permit = shared
            .wake
            .live
            .admit_if(after.is_some() && !wait.is_zero())?;
        shared.wake.live.tree_after(after, wait).await?
    };
    let crate::runtime::reader::Reply { value } = reader.request(&hub::Caller::Human, args).await?;
    Ok(api::value(api::TreeReply {
        version,
        tree: value,
    }))
}

/// Open the live channel before the snapshot, so no item falls between them.
async fn attach(shared: &Shared, reader: &Reader, args: RequestEnvelope) -> Result<Value> {
    let Request::Attach(attach) = &args.request else {
        return Err(invalid_request());
    };
    let target = attach
        .target
        .required(api::InputContext::Attach, "/target")?
        .to_owned();
    let (live, created) = shared.wake.live.open(&target)?;
    match reader.request(&hub::Caller::Human, args).await {
        Ok(crate::runtime::reader::Reply { value }) => Ok(api::value(api::AttachReply {
            live,
            snapshot: value,
        })),
        Err(error) => {
            if created {
                shared.wake.live.discard(&target, live.generation);
            }
            Err(error)
        }
    }
}

async fn finish_purge(shared: &Shared, reader: &Reader) -> Result<()> {
    while finish_purge_batch(shared, reader).await? {}
    Ok(())
}

/// Remove at most one committed batch. GC clients yield between cleanup requests.
async fn finish_purge_batch(shared: &Shared, reader: &Reader) -> Result<bool> {
    let attachment_pending = shared.cleanup_attachments().await?;
    let (dir, ids, owner) = shared
        .with(|core| {
            let super::super::store::ArtifactDeletions { dir, ids } =
                core.store().pending_artifact_deletions()?;
            Ok((dir, ids, core.daemon_lock().clone()))
        })
        .await?;
    if ids.is_empty() {
        return Ok(attachment_pending);
    }
    // Fetch precedes the barrier: every older artifact read finishes before unlink.
    reader.barrier().await?;
    let removed = tokio::task::spawn_blocking(move || {
        // Blocking jobs retain ownership even if their control handler is cancelled.
        let _owner = owner;
        super::super::store::unlink_artifacts(&dir, &ids)?;
        Ok::<_, Error>(ids)
    })
    .await
    .map_err(|error| {
        Error::state_io("Artifact cleanup did not finish; retry the purge request.")
            .with_source(error)
    })??;
    shared
        .with(move |core| {
            core.store().acknowledge_artifact_deletions(&removed)?;
            Ok(attachment_pending || !core.store().pending_artifact_deletions()?.ids.is_empty())
        })
        .await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Test fixtures fail immediately on setup/assertion errors.
mod tests;
