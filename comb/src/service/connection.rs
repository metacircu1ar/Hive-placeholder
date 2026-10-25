//! Connection admission, frame handling and bounded shutdown.
use super::{
    Arc, BufReader, Duration, Error, Reply, Request, Result, Semaphore, Server, State,
    UnixListener, framing, input, limits,
    observations::{self, Loss},
    queries,
};
use std::future::Future;
use tokio::{
    net::UnixStream,
    sync::OwnedSemaphorePermit,
    task::{JoinHandle, JoinSet},
};

#[cfg(test)]
mod tests;

impl Server {
    pub async fn run(self, shutdown: impl Future<Output = ()>) -> Result<()> {
        self.run_with_deadline(shutdown, limits::REQUEST).await
    }

    pub(super) async fn run_with_deadline(
        self,
        shutdown: impl Future<Output = ()>,
        deadline: Duration,
    ) -> Result<()> {
        let Self {
            state,
            listener,
            queries,
            _socket: mut socket_guard,
        } = self;
        let listener = UnixListener::from_std(listener)?;
        let worker = state.model.as_ref().map(|_| {
            let state = state.clone();
            tokio::spawn(async move { state.backfill().await })
        });
        let queries = tokio::spawn(queries::serve(state.clone(), queries));
        let mut handlers = JoinSet::new();
        let result = accept_loop(&listener, &state, &mut handlers, shutdown, deadline).await;
        shutdown_handlers(worker, queries, &mut handlers).await;
        state.observations.finish();
        drop(listener);
        let cleanup = socket_guard.cleanup();
        match result {
            Ok(()) => cleanup,
            Err(error) => Err(super::auth::with_cleanup(error, cleanup)),
        }
    }
}

async fn accept_loop(
    listener: &UnixListener,
    state: &Arc<State>,
    handlers: &mut JoinSet<()>,
    shutdown: impl Future<Output = ()>,
    deadline: Duration,
) -> Result<()> {
    // Independent frame headroom remains available while semantic slots wait.
    let frames = Arc::new(Semaphore::new(limits::FRAME_SLOTS));
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => return Ok(()),
            completed = handlers.join_next(), if !handlers.is_empty() => {
                if let Some(result) = completed {
                    observations::joined(result, "connection");
                }
            },
            accepted = listener.accept() => admit(accepted, state, &frames, handlers, deadline)?,
        }
    }
}

fn admit(
    accepted: std::io::Result<(UnixStream, tokio::net::unix::SocketAddr)>,
    state: &Arc<State>,
    frames: &Arc<Semaphore>,
    handlers: &mut JoinSet<()>,
    deadline: Duration,
) -> Result<()> {
    let (stream, _) = accepted?;
    let Ok(permit) = frames.clone().try_acquire_owned() else {
        state.observations.lost(Loss::FrameCapacity, None);
        return Ok(());
    };
    handlers.spawn(serve_connection(state.clone(), stream, permit, deadline));
    Ok(())
}

async fn serve_connection(
    state: Arc<State>,
    stream: UnixStream,
    permit: OwnedSemaphorePermit,
    deadline: Duration,
) {
    let _permit = permit;
    let mut stream = BufReader::new(stream);
    let result = match read_request(&mut stream).await {
        Ok(Some(request)) => handle_request(&state, request, deadline).await,
        Ok(None) => return,
        Err(error) => Err(error),
    };
    if let Err(error) = &result {
        error.log_diagnostic();
    }
    let reply = Reply::from_result(result);
    match tokio::time::timeout(
        limits::OUTPUT,
        framing::write(stream.get_mut(), &reply, framing::RESPONSE_BYTES),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            let loss = if observations::peer_gone(&error) {
                Loss::PeerGone
            } else {
                Loss::ReplyFailure
            };
            state.observations.lost(loss, Some(&error));
        }
        Err(cause) => state.observations.lost(Loss::ReplyTimeout, Some(&cause)),
    }
}

async fn read_request(stream: &mut BufReader<UnixStream>) -> Result<Option<Request>> {
    match tokio::time::timeout(
        limits::FRAME_READ,
        framing::read(stream, framing::REQUEST_BYTES),
    )
    .await
    {
        Ok(Ok(Some(bytes))) => input::request(&bytes).map(Some),
        Ok(Ok(None)) => Ok(None),
        Ok(Err(error)) => Err(error),
        Err(cause) => Err(Error::request_frame_timeout().caused_by(cause)),
    }
}

async fn handle_request(
    state: &Arc<State>,
    request: Request,
    deadline: Duration,
) -> Result<serde_json::Value> {
    let id = request.operation.request_id().map(str::to_owned);
    let lost_reply = request.operation.lost_reply();
    match tokio::time::timeout(deadline, state.handle(request)).await {
        Ok(result) => result.map_err(|mut error| {
            error.request_id = id;
            error
        }),
        Err(cause) => Err(lost_reply.caused_by(cause)),
    }
}

async fn shutdown_handlers(
    worker: Option<JoinHandle<()>>,
    queries: JoinHandle<()>,
    handlers: &mut JoinSet<()>,
) {
    if let Some(worker) = &worker {
        worker.abort();
    }
    queries.abort();
    // Stop accepting first; admitted blocking jobs retain their own permits
    // after this bounded drain drops the client observations.
    let drained = tokio::time::timeout(limits::DRAIN, async {
        // abort() only requests cancellation. Reap the async owners before
        // returning so an idle service's lock is available for immediate reopen.
        if let Some(worker) = worker {
            observations::joined(worker.await, "backfill");
            tracing::info!(name: "comb.backfill_stopped", confirmed = true);
        }
        observations::joined(queries.await, "query");
        tracing::info!(name: "comb.query_worker_stopped", confirmed = true);
        while let Some(result) = handlers.join_next().await {
            observations::joined(result, "connection");
        }
    })
    .await;
    if let Err(cause) = drained {
        tracing::warn!(name: "comb.drain_timeout",
             count = handlers.len(),
             error = &cause as &(dyn std::error::Error + 'static),
             decision = "abort_observations");
    }
    handlers.abort_all();
}
