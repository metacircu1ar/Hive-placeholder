//! External stdio MCP adapter. Never included in provider-owned agent catalogs.
use crate::{
    calls,
    error::{Error, Result},
    runtime::domain::wire_enum,
};
use serde_json::Value;
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    result,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Stdout},
    sync::mpsc,
    task::{JoinError, JoinHandle},
};
use uuid::Uuid;
use wire::{Frame, Message, RequestId, ResultBody};

mod catalog;
mod tools;
mod wire;
// Bound simultaneously active external observations.
const MAX_CALLS: usize = 16;
// Bound an external JSON-RPC frame before deserialization.
const MAX_FRAME_BYTES: u64 = 1024 * 1024;
const FRAME_READ_BYTES: u64 = MAX_FRAME_BYTES + 1;
// Allow queued output to drain briefly; a blocked client must not hold shutdown.
const OUTPUT_TIMEOUT: Duration = Duration::from_millis(500);
// Bound queued replies and per-call progress updates independently.
const OUTPUT_QUEUE: usize = 32;
const UPDATE_QUEUE: usize = 64;
#[cfg(test)]
const PROTOCOL: &str = crate::compatibility::EXTERNAL_MCP[0];

wire_enum!(McpMethod {
    Initialize => "initialize", Initialized => "notifications/initialized", Ping => "ping",
    Cancelled => "notifications/cancelled", ToolsList => "tools/list", ToolsCall => "tools/call",
});

struct Active {
    generation: String,
    task: JoinHandle<()>,
}
impl Drop for Active {
    fn drop(&mut self) {
        self.task.abort();
    }
}
struct Update {
    key: String,
    generation: String,
    value: Message,
    finished: bool,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum McpPhase {
    #[default]
    New,
    Initialized,
    Ready,
}

#[derive(Default)]
struct Server {
    phase: McpPhase,
    active: HashMap<String, Active>,
}

pub(crate) async fn serve(root: PathBuf) -> Result<()> {
    let (output, mut frames) = mpsc::channel::<Message>(OUTPUT_QUEUE);
    let mut writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(frame) = frames.recv().await {
            write(&mut stdout, frame).await?;
        }
        Ok(())
    });
    let result = tokio::select! {
        result = receive(root, &output) => result,
        result = &mut writer => return writer_result(result),
    };
    drop(output);
    // Allow queued replies to drain on EOF, without letting an unread pipe hold
    // the process alive. The CLI also uses bounded runtime shutdown for stdio's
    // non-cancellable blocking I/O tasks.
    let cleanup = match tokio::time::timeout(OUTPUT_TIMEOUT, &mut writer).await {
        Ok(joined) => writer_result(joined),
        Err(error) => {
            let task_id = writer.id().to_string();
            tracing::warn!(name: "mcp.writer_abandoned",
                 task_id = task_id,
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "abort_observer");
            writer.abort();
            Ok(())
        }
    };
    crate::observability::settle(result, cleanup, "mcp_output_drain")
}

fn writer_result(result: result::Result<Result<()>, JoinError>) -> Result<()> {
    result.map_err(|cause| {
        let task_id = cause.id().to_string();
        tracing::warn!(name: "mcp.writer_join_failed",
             task_id = task_id,
             error = &cause as &(dyn std::error::Error + 'static),
             decision = "close_transport");
        Error::state_io_from(io::Error::other(cause))
    })?
}

async fn receive(root: PathBuf, output: &mpsc::Sender<Message>) -> Result<()> {
    let mut input = BufReader::new(tokio::io::stdin());
    let (updates, mut received) = mpsc::channel::<Update>(UPDATE_QUEUE);
    let mut server = Server::default();
    // Keep the read future alive across progress updates; cancellation of read_line
    // could otherwise discard a partially received JSON-RPC frame.
    loop {
        let mut bytes = Vec::new();
        let mut limited = (&mut input).take(FRAME_READ_BYTES);
        let read = limited.read_until(b'\n', &mut bytes);
        tokio::pin!(read);
        let count = loop {
            tokio::select! {
                count = &mut read => break count.map_err(Error::state_io_from)?,
                Some(update) = received.recv() => server.progress(output, update).await?,
            }
        };
        if count == 0 {
            break;
        }
        if count as u64 > MAX_FRAME_BYTES {
            return Err(Error::output_limit("MCP request exceeded one MiB."));
        }
        let reply = match wire::parse(&bytes) {
            Ok(frame) => server.handle(frame, &root, &updates),
            Err(error) => Some(error),
        };
        if let Some(reply) = reply {
            enqueue(output, reply, true).await?;
        }
    }
    // Dropping Active aborts observation and closes sockets, never sends cancellation.
    Ok(())
}

async fn enqueue(output: &mpsc::Sender<Message>, value: Message, required: bool) -> Result<()> {
    // Send errors own unsent frames. Drop them instead of retaining caller payloads
    // in a failure that can outlive this bounded queue.
    match output.try_send(value) {
        Ok(()) => Ok(()),
        Err(mpsc::error::TrySendError::Full(_)) if !required => {
            crate::observability::lost_reply(crate::observability::ReplyOwner::Control, None);
            Ok(())
        }
        Err(mpsc::error::TrySendError::Full(value)) => {
            // Buffered stdin can fill this queue before the writer gets a turn.
            // Let a healthy reader drain a burst, but bound an unread pipe's wait.
            match tokio::time::timeout(OUTPUT_TIMEOUT, output.send(value)).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(cause)) => {
                    crate::observability::lost_reply(
                        crate::observability::ReplyOwner::Control,
                        None,
                    );
                    Err(blocked_output().with_send_source(cause))
                }
                Err(cause) => {
                    crate::observability::lost_reply(
                        crate::observability::ReplyOwner::Control,
                        Some(&cause),
                    );
                    Err(blocked_output().with_source(cause))
                }
            }
        }
        Err(cause) => {
            crate::observability::lost_reply(crate::observability::ReplyOwner::Control, None);
            Err(blocked_output().with_try_send_source(cause))
        }
    }
}

fn blocked_output() -> Error {
    Error::output_limit(
        "MCP output is blocked or closed; reconnect and resume by message/request ID.",
    )
}

#[expect(
    clippy::expect_used,
    reason = "Typed MCP frames contain only JSON-compatible data."
)]
async fn write(output: &mut Stdout, value: Message) -> Result<()> {
    let mut bytes = serde_json::to_vec(&value).expect("MCP JSON serializes");
    bytes.push(b'\n');
    output
        .write_all(&bytes)
        .await
        .map_err(Error::state_io_from)?;
    output.flush().await.map_err(Error::state_io_from)
}

impl Server {
    async fn progress(&mut self, output: &mpsc::Sender<Message>, update: Update) -> Result<()> {
        if self
            .active
            .get(&update.key)
            .is_some_and(|a| a.generation == update.generation)
        {
            enqueue(output, update.value, update.finished).await?;
            if update.finished {
                self.active.remove(&update.key);
            }
        }
        Ok(())
    }

    fn handle(
        &mut self,
        frame: Frame,
        root: &Path,
        updates: &mpsc::Sender<Update>,
    ) -> Option<Message> {
        if frame.jsonrpc != "2.0" {
            return Some(Message::error(
                None,
                wire::INVALID_REQUEST,
                wire::INVALID_ENVELOPE,
            ));
        }
        let Some(id) = frame.id else {
            self.notification(&frame);
            return None;
        };
        if self.active.contains_key(&id.key()) {
            return Some(Message::error(
                None,
                wire::INVALID_REQUEST,
                "Request ID is already active.",
            ));
        }
        let result = match frame.method {
            McpMethod::Initialize if self.phase == McpPhase::New => {
                let Some(protocol) = wire::initialize_protocol(frame.params) else {
                    return Some(Message::error(
                        Some(id),
                        wire::INVALID_PARAMS,
                        "Invalid initialize parameters.",
                    ));
                };
                self.phase = McpPhase::Initialized;
                ResultBody::initialize(protocol)
            }
            McpMethod::Ping if self.phase != McpPhase::New => ResultBody::empty(),
            _ if self.phase != McpPhase::Ready => {
                return Some(Message::error(
                    Some(id),
                    wire::SERVER_ERROR,
                    "Initialize this MCP session first.",
                ));
            }
            McpMethod::ToolsList => ResultBody::catalog(tools::catalog()),
            McpMethod::ToolsCall => return self.tool_call(root, id, frame.params, updates),
            _ => {
                return Some(Message::error(
                    Some(id),
                    wire::METHOD_NOT_FOUND,
                    "Unknown method.",
                ));
            }
        };
        Some(Message::reply(id, result))
    }

    fn notification(&mut self, frame: &Frame) {
        if frame.method == McpMethod::Initialized && self.phase != McpPhase::New {
            self.phase = McpPhase::Ready;
        }
        if frame.method == McpMethod::Cancelled
            && let Some(id) = wire::cancellation(&frame.params)
        {
            self.active.remove(&id.key());
        }
    }

    fn tool_call(
        &mut self,
        root: &Path,
        id: RequestId,
        params: Value,
        updates: &mpsc::Sender<Update>,
    ) -> Option<Message> {
        if self.active.len() >= MAX_CALLS {
            return Some(Message::error(
                Some(id),
                wire::SERVER_ERROR,
                "Observer capacity reached; retry later.",
            ));
        }
        match tools::parse(params) {
            Ok(call) => {
                self.start(root.to_owned(), id, call, updates.clone());
                None
            }
            Err(tools::ParseError::Protocol(error)) => Some(Message::input_error(id, error)),
            Err(tools::ParseError::Arguments(error)) => Some(Message::reply(
                id,
                ResultBody::tool(wire::tool_error(error.into()), true),
            )),
        }
    }

    fn start(
        &mut self,
        root: PathBuf,
        id: RequestId,
        call: tools::ToolCall,
        updates: mpsc::Sender<Update>,
    ) {
        let key = id.key();
        let generation = Uuid::new_v4().to_string();
        let task_key = key.clone();
        let task_generation = generation.clone();
        let task = tokio::spawn(async move {
            let mut observations = 0_u64;
            let progress_token = call.progress_token.clone();
            let progress = |result: &calls::CallResult| {
                observations += 1;
                if let Some(token) = &progress_token {
                    #[expect(
                        clippy::expect_used,
                        reason = "CallResult fields and opaque Value payloads are \
                            JSON-compatible."
                    )]
                    let message =
                        serde_json::to_string(result).expect("serializable call progress");
                    // Best-effort progress drops a full/closed queue's owned frame immediately.
                    crate::observability::reply(
                        updates.try_send(Update {
                            key: task_key.clone(),
                            generation: task_generation.clone(),
                            finished: false,
                            value: Message::progress(token.clone(), observations, message),
                        }),
                        crate::observability::ReplyOwner::Control,
                    );
                }
            };
            let tools::ToolResult { value, error } = call.execute(&root, progress).await;
            let response = Message::reply(id, ResultBody::tool(value, error));
            // A closed observer drops its unsent response rather than retaining the payload.
            crate::observability::reply(
                updates
                    .send(Update {
                        key: task_key,
                        generation: task_generation,
                        value: response,
                        finished: true,
                    })
                    .await,
                crate::observability::ReplyOwner::Control,
            );
        });
        self.active.insert(key, Active { generation, task });
    }
}

#[cfg(test)]
mod tests;
