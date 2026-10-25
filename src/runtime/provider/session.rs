//! Agent-facing session ownership, requests, retirement and release.
use super::protocol::{ClientMethod, IncomingFrame, Model, Response, outgoing};
use super::{
    error::{protocol, unavailable},
    pool::Pool,
    rpc::Shutdown,
    shard::{Cmd, Frame, Shard, Started},
};
use crate::runtime::provider::ShutdownMethod;
use crate::runtime::provider::error::{TERMINAL_CLEANUP_TIMEOUT, THREAD_UNLOAD_TIMEOUT};
use crate::{
    error::{Error, Result},
    runtime::{locks::MutexExt, scheduler::Activity},
};
use std::future::Future;
use std::sync::atomic::AtomicBool;
use std::{
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, watch};

// How long session retirement waits for an interrupted turn to finish.
const SESSION_INTERRUPT_GRACE: Duration = Duration::from_secs(10);
// Interval between active-turn checks while session retirement awaits interruption.
const SESSION_INTERRUPT_POLL: Duration = Duration::from_millis(25);
// How long session retirement waits for background terminals to disappear.
const SESSION_TERMINAL_GRACE: Duration = Duration::from_secs(10);
// Interval between background-terminal checks during session retirement.
const SESSION_TERMINAL_POLL: Duration = Duration::from_millis(100);
// The configured resident-session ceiling is 1,024, so one maximum-size native
// page must contain the full loaded-thread inventory.
const LOADED_THREAD_LIMIT: u32 = 1024;
const SESSION_UNLOAD_GRACE: Duration = Duration::from_secs(10);
const SESSION_UNLOAD_POLL: Duration = Duration::from_millis(25);

pub(crate) struct Session {
    pub(super) pool: Pool,
    pub(super) shard: Arc<Shard>,
    pub(super) agent: String,
    pub(super) events: mpsc::Receiver<Frame>,
    pub(super) failure: watch::Receiver<Option<Error>>,
    pub started: Started,
    pub thread: Option<String>,
    pub(super) released: bool,
}
enum SessionReceive {
    Failure(std::result::Result<(), watch::error::RecvError>),
    Frame(Option<Frame>),
}
impl Session {
    pub(crate) fn mark_ready(&self) {
        let _ = self.shard.ready_since.set(Instant::now());
    }
    pub(crate) fn codex(&self) -> bool {
        self.shard.backend.multiplexed()
    }
    pub(crate) fn provider_id(&self) -> &str {
        &self.shard.id
    }
    pub(crate) fn provider_version(&self) -> Option<&str> {
        self.shard
            .compatibility
            .as_ref()
            .map(|evidence| evidence.observation.version.as_str())
    }
    pub(crate) fn draining(&self) -> bool {
        self.shard.draining()
    }
    /// Fence this process before releasing an owner with unresolved requests.
    pub(crate) fn drain(&self) {
        self.shard.begin_drain();
        self.pool.wake.all();
    }
    pub(crate) async fn quarantine(&self, thread: &str) -> Result<()> {
        self.shard.begin_drain();
        self.pool.wake.all();
        let (tx, rx) = oneshot::channel();
        self.shard
            .command(Cmd::Quarantine(thread.into(), tx), true)
            .await?;
        rx.await.map_err(|cause| unavailable().with_source(cause))
    }
    // Keep this API lazy: catalog access occurs when the returned future is polled.
    #[allow(clippy::unused_async, clippy::unused_async_trait_impl)]
    pub(crate) async fn catalog(&self) -> Result<Vec<Model>> {
        self.shard
            .catalog
            .get()
            .cloned()
            .unwrap_or_else(|| Err(unavailable()))
    }
    pub(crate) fn activity(&self, class: usize, busy: bool, waiting: bool, touch: bool) {
        let mut occupancy = self.shard.occupancy();
        // Placement is monotonic for this resident session: coordinator/mail
        // promotion survives turn completion (whose neutral class is 2).
        let old = occupancy.activity.get(&self.agent).copied();
        occupancy.activity.insert(
            self.agent.clone(),
            Activity {
                class: old.map_or(class, |a| a.class.min(class)),
                busy,
                waiting,
            },
        );
        if touch {
            self.shard
                .touch(self.pool.clock.fetch_add(1, Ordering::Relaxed));
        }
    }
    pub(crate) async fn call(&self, request: outgoing::OutgoingRequest) -> Result<Response> {
        self.pending_call(request).await
    }
    /// Own the request future so a worker can service events and controls during
    /// both channel backpressure and the provider acknowledgement wait.
    pub(crate) fn pending_call(
        &self,
        request: outgoing::OutgoingRequest,
    ) -> impl Future<Output = Result<Response>> + Send + 'static {
        self.observed_call(request, None)
    }
    pub(crate) fn observed_call(
        &self,
        request: outgoing::OutgoingRequest,
        queued: Option<Arc<AtomicBool>>,
    ) -> impl Future<Output = Result<Response>> + Send + 'static {
        let method = request.method();
        let priority = method.is_cleanup();
        let failure = self.failure.borrow().clone();
        let shard = self.shard.clone();
        let agent = self.agent.clone();
        async move {
            if !priority && let Some(error) = failure {
                return Err(method.submission_error(shard.backend.multiplexed(), error));
            }
            let steering = method == ClientMethod::TurnSteer;
            if steering && shard.steering_unsupported() {
                return Err(Error::unsupported_operation(
                    "Provider does not support steering.",
                ));
            }
            let (tx, rx) = oneshot::channel();
            shard
                .command(Cmd::Call(agent, request, tx), priority)
                .await?;
            if let Some(queued) = queued {
                queued.store(true, Ordering::Release);
            }
            let result = rx.await.map_err(|cause| unavailable().with_source(cause))?;
            // Only TurnSteer's exact -32601 classification enables this cache.
            if steering && matches!(result, Err(Error::UnsupportedOperation { .. })) {
                shard.reject_steering();
            }
            result
        }
    }
    pub(crate) async fn commit_branch(&self, turn: &str) -> Result<()> {
        if self.codex() {
            return Ok(());
        }
        let (tx, rx) = oneshot::channel();
        self.shard
            .command(Cmd::CommitBranch(turn.into(), tx), true)
            .await?;
        rx.await.map_err(|cause| unavailable().with_source(cause))?
    }
    pub(crate) async fn send(&self, value: outgoing::OutgoingFrame) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.shard.command(Cmd::Send(value, tx), true).await?;
        rx.await.map_err(|cause| unavailable().with_source(cause))?
    }
    #[cfg(feature = "comb")]
    pub(crate) async fn forget_comb_turn(&self, turn: &str) -> Result<()> {
        if self.codex() {
            return Ok(());
        }
        let (tx, rx) = oneshot::channel();
        self.shard
            .command(Cmd::ForgetCombTurn(turn.into(), tx), true)
            .await?;
        rx.await.map_err(|cause| unavailable().with_source(cause))
    }
    #[cfg(feature = "comb")]
    pub(crate) async fn forget_tool(&self, id: serde_json::Value) -> Result<()> {
        if self.codex() {
            return Ok(());
        }
        let (tx, rx) = oneshot::channel();
        self.shard.command(Cmd::ForgetTool(id, tx), true).await?;
        rx.await.map_err(|cause| unavailable().with_source(cause))?
    }
    pub(crate) async fn recv(&mut self) -> Result<IncomingFrame> {
        loop {
            let failure = self.failure.borrow().clone();
            if let Some(error) = failure {
                self.events.close();
                while self.events.try_recv().is_ok() {}
                return Err(error);
            }
            let event = tokio::select! {
                biased;
                changed = self.failure.changed() => SessionReceive::Failure(changed),
                frame = self.events.recv() => SessionReceive::Frame(frame),
            };
            match event {
                SessionReceive::Failure(changed) => {
                    if let Err(cause) = changed
                        && self.failure.borrow().is_none()
                    {
                        if self.shard.final_events() {
                            // The route is closed and its queue is bounded. Discard
                            // cancelled callbacks, retaining terminal observations.
                            while let Ok(frame) = self.events.try_recv() {
                                if !frame.value.is_request() && !frame.value.is_response() {
                                    return Ok(frame.value.clone());
                                }
                            }
                        }
                        return Err(unavailable().with_source(cause));
                    }
                }
                SessionReceive::Frame(frame) => {
                    if let Some(error) = self.failure.borrow().clone() {
                        return Err(error);
                    }
                    return frame.map(|f| f.value.clone()).ok_or_else(unavailable);
                }
            }
        }
    }
    async fn active(&self, thread: &str) -> Result<Option<String>> {
        let (tx, rx) = oneshot::channel();
        self.shard
            .command(Cmd::Inspect(thread.into(), tx), true)
            .await?;
        rx.await.map_err(|cause| unavailable().with_source(cause))?
    }
    pub(crate) async fn interrupt(&self, thread: &str, turn: &str) -> Result<()> {
        match self.call(outgoing::interrupt(thread, turn)).await {
            Ok(_) | Err(Error::ProviderRequestFailed { .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }
    /// Request cleanup and observe an empty inventory (not an admission fence).
    pub(crate) async fn clean_terminals(&self, thread: &str) -> Result<()> {
        let deadline = Instant::now() + SESSION_TERMINAL_GRACE;
        tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            self.call(outgoing::terminal_clean(thread)),
        )
        .await
        .map_err(|cause| Error::provider_timeout(TERMINAL_CLEANUP_TIMEOUT).with_source(cause))??;
        loop {
            let frame_value = tokio::time::timeout_at(
                tokio::time::Instant::from_std(deadline),
                self.call(outgoing::terminal_list(thread)),
            )
            .await
            .map_err(|cause| {
                Error::provider_timeout(TERMINAL_CLEANUP_TIMEOUT).with_source(cause)
            })??;
            if frame_value.terminals_empty()? {
                break;
            }
            if Instant::now() >= deadline {
                return Err(protocol());
            }
            tokio::time::sleep(SESSION_TERMINAL_POLL).await;
        }
        Ok(())
    }
    async fn retire(&mut self) -> Result<()> {
        let Some(thread) = self.thread.clone() else {
            return Err(protocol());
        };
        if let Some(turn) = self.active(&thread).await? {
            self.interrupt(&thread, &turn).await?;
            let deadline = Instant::now() + SESSION_INTERRUPT_GRACE;
            while self.active(&thread).await?.is_some() {
                if Instant::now() >= deadline {
                    return Err(Error::interrupt_timeout(
                        "Provider did not confirm interruption.",
                    ));
                }
                // Cleanup is independent of event delivery: a failed route is a
                // bounded sink, but RPC still tracks matching completion.
                while self.events.try_recv().is_ok() {}
                tokio::time::sleep(SESSION_INTERRUPT_POLL).await;
            }
            self.observe_terminal_cleanup(&thread, &turn).await?;
        }
        self.clean_terminals(&thread).await?;
        let frame_value = self.call(outgoing::thread_unsubscribe(&thread)).await?;
        if !matches!(
            frame_value.status(),
            Some(
                super::protocol::UnsubscribeStatus::Unsubscribed
                    | super::protocol::UnsubscribeStatus::NotSubscribed
                    | super::protocol::UnsubscribeStatus::NotLoaded
            )
        ) {
            return Err(protocol());
        }
        self.confirm_unloaded(&thread).await?;
        let (tx, rx) = oneshot::channel();
        self.shard.command(Cmd::Retire(thread, tx), true).await?;
        rx.await.map_err(|cause| unavailable().with_source(cause))?
    }
    async fn confirm_unloaded(&self, thread: &str) -> Result<()> {
        let deadline = Instant::now() + SESSION_UNLOAD_GRACE;
        loop {
            let response = tokio::time::timeout_at(
                tokio::time::Instant::from_std(deadline),
                self.call(outgoing::loaded_thread_page(LOADED_THREAD_LIMIT, None)),
            )
            .await
            .map_err(|cause| Error::provider_timeout(THREAD_UNLOAD_TIMEOUT).with_source(cause))??;
            let (loaded, cursor) = response.loaded_threads()?;
            if cursor.is_some() {
                // More than the configured maximum means this is not a complete
                // inventory and therefore cannot prove absence.
                return Err(protocol());
            }
            if !loaded.contains(&thread) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Error::provider_timeout(THREAD_UNLOAD_TIMEOUT));
            }
            tokio::time::sleep(SESSION_UNLOAD_POLL).await;
        }
    }
    pub(crate) async fn close(&mut self) -> Shutdown {
        let mut clean = true;
        if self.codex()
            && self.pool.settings.sessions_per_codex_process > 1
            && self.thread.is_some()
            && let Err(error) = self.retire().await
        {
            tracing::warn!(name: "provider.retire_failed",
                 provider_id = self.shard.id.as_str(),
                 agent_id = self.agent.as_str(),
                 code = error.code(),
                 error = &error as &(dyn std::error::Error + 'static),
                 decision = "drain_provider");
            clean = false;
        }
        if !clean {
            self.shard.begin_drain();
            self.pool.wake.all();
        }
        let last = self.pool.release_user(
            &self.shard,
            &self.agent,
            clean && self.codex() && self.thread.is_some(),
        );
        let result = self.release(last, clean).await;
        if result.is_none() {
            // Shutdown closes command admission before it finishes process proof
            // and durable receipts. A late Release must await that final report.
            self.shard.wait_exited().await;
        }
        self.released = true;
        result
            .or_else(|| self.shard.report.lock_or_panic().clone())
            .unwrap_or(Shutdown {
                method: ShutdownMethod::Uncertain,
                confirmed: false,
                quiescent: false,
                reaped: false,
                exit_code: None,
            })
    }

    async fn release(&self, last: bool, clean: bool) -> Option<Shutdown> {
        let (tx, rx) = oneshot::channel();
        if let Err(error) = self
            .shard
            .command(
                Cmd::Release {
                    agent: self.agent.clone(),
                    last,
                    clean,
                    reply: tx,
                },
                true,
            )
            .await
        {
            crate::observability::lost_reply(
                crate::observability::ReplyOwner::Provider,
                Some(&error),
            );
            return None;
        }
        // A lost reply falls through to the shard's final exit report, never inferred cleanup.
        match rx.await {
            Ok(report) => Some(report),
            Err(error) => {
                crate::observability::lost_reply(
                    crate::observability::ReplyOwner::Provider,
                    Some(&error),
                );
                None
            }
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if !self.released {
            self.pool.quarantine(&self.shard);
            self.shard.mark_draining();
            self.shard.request_stop();
            self.shard.wake_owner();
            self.pool.wake.all();
        }
    }
}
