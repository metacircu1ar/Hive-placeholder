//! Drive bounded shard commands, provider frames, and request deadlines.
use super::{
    AsFd, COMMAND_DRAIN_BUDGET, ClientMethod, Duration, Error, EventKind, FRAME_DRAIN_BUDGET,
    Instant, InterruptedTurn, MutexExt, NORMAL_RPC_WINDOW, PRIORITY_RPC_WINDOW, Pending,
    REQUEST_ACK_TIMEOUT, REQUEST_TOMBSTONE_LIMIT, Read, Result, SHARD_CLOSE_GRACE,
    SHARD_IDLE_POLL_TIMEOUT, ShardLoop, WAKE_READ_BYTES, bootstrap, in_flight, outgoing, protocol,
};
use crate::observability::{ReplyOwner, reply as observe_reply};
use crate::runtime::api::{self, events};
use crate::runtime::auth_fallback::AuthReason;
use crate::runtime::auth_fallback::AvailabilityState;
use crate::runtime::auth_fallback::signals;
use crate::runtime::provider::error::ACK_TIMEOUT;
use crate::state::ProviderEvidence;

impl ShardLoop<'_> {
    pub(super) fn run(
        &mut self,
        recorded: Result<()>,
        evidence: &ProviderEvidence,
        credential: &crate::auth::Credential,
    ) -> Result<()> {
        recorded?;
        if self.pool.settings.live_stream {
            self.rpc.stream_live(self.pool.wake.clone());
        }
        let catalog =
            bootstrap(self.rpc, credential, &self.shard.lease.provider_home()).and_then(|models| {
                evidence.account(self.rpc.account_identity.clone())?;
                Ok(models)
            });
        let failed = catalog.as_ref().err().cloned();
        if let (Ok(models), Some(binary)) = (&catalog, self.shard.binary_identity.get())
            && binary.unchanged()
        {
            self.pool.hub.catalogs.observe(
                self.shard.lease.generation(),
                credential.authority().cloned(),
                binary.clone(),
                models.clone(),
            );
        }
        let _ = self.shard.catalog.set(catalog);
        self.shard.mark_started();
        self.pool.changed.notify_waiters();
        if let Some(error) = failed {
            if matches!(error, Error::AuthenticationRequired { .. }) {
                self.record_limit(
                    signals::Signal {
                        state: AvailabilityState::Unavailable,
                        ..signals::Signal::limited(AuthReason::Authentication)
                    },
                    None,
                )?;
            }
            return Err(error);
        }
        if let Some(reply) = self.rpc.initialization.take() {
            let provider = self.shard.id.clone();
            let compatibility = self
                .shard
                .compatibility
                .as_ref()
                .map(|proof| proof.observation.clone());
            self.pool.hub.blocking_with(move |core| {
                core.store().provider_event(
                    &provider,
                    crate::runtime::domain::ProviderEventKind::Initialized,
                    api::value(events::ProviderInitialized {
                        compatibility,
                        reply,
                    }),
                )
            })?;
        }
        if self.shard.backend.multiplexed() && self.shard.key.is_none() {
            self.read_limits()?;
        }
        let validated = Instant::now();
        self.event_loop(validated)
    }

    /// Read a subscription Codex login's limits once at start: for
    /// availability and for `/usage`.
    fn read_limits(&mut self) -> Result<()> {
        let auth = self.shard.auth.clone();
        let generation = self.shard.lease.generation();
        let revision = self.pool.hub.blocking_with(move |core| {
            Ok(core.store().availability(&auth, generation)?.revision)
        })?;
        if let Ok(response) = self
            .rpc
            .call(outgoing::rate_limits_read())
            .inspect_err(|error| {
                tracing::debug!(name: "auth.probe_decision",
                 provider_id = self.shard.id.as_str(),
                 phase = "rate_limits_read",
                 error = error as &(dyn std::error::Error + 'static),
                 decision = "retain_availability");
            })
        {
            let raw = response.into_raw();
            self.pool.usage.codex(
                &self.shard.auth,
                generation,
                &raw["rateLimits"],
                crate::state::now(),
            );
            if let Ok(read) =
                serde_json::from_value::<signals::CodexRead>(raw).inspect_err(|error| {
                    super::super::protocol::lenient::drift("codex_limits_read", Some(error));
                })
            {
                self.limits.merge(read.rate_limits.clone());
                let auth = self.shard.auth.clone();
                let provider = self.shard.id.clone();
                self.pool.hub.blocking_with(move |core| {
                    core.auth_live_read(&auth, generation, &provider, (revision, None), read)
                })?;
            }
        }
        Ok(())
    }

    pub(super) fn event_loop(&mut self, mut validated: Instant) -> Result<()> {
        // Commands, observed frames and expiry transitions must retain this order.
        loop {
            if self.expire_spare() || self.shard.stop() {
                return Ok(());
            }
            self.drain_wake()?;
            if self.shard.abandoned_attach() {
                self.closing
                    .get_or_insert_with(|| Instant::now() + SHARD_CLOSE_GRACE);
            }
            self.drain_commands(&mut validated)?;
            self.drain_frames()?;
            self.expire_requests()?;
            self.interrupt_failed_owners()?;
            if self
                .closing
                .is_some_and(|d| !self.rpc.unresolved_lifecycle() || Instant::now() >= d)
            {
                return Ok(());
            }
            self.wait_for_input()?;
        }
    }

    fn drain_wake(&mut self) -> Result<()> {
        let mut buffer = [0; WAKE_READ_BYTES];
        loop {
            match self.wake.read(&mut buffer) {
                Ok(0) => return Ok(()),
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(Error::state_io_from(error)),
            }
        }
    }

    pub(super) fn expire_spare(&self) -> bool {
        // Fence an empty spare before releasing the registry lock so acquisition
        // cannot attach to a process already exiting.
        let _registry = self.pool.registry.lock_or_panic();
        let occupancy = self.shard.occupancy();
        let unused = occupancy.slots.is_empty();
        let idle = occupancy.idle_since;
        let expire = unused
            && (self.shard.draining()
                || self.shard.created.elapsed()
                    >= Duration::from_secs(self.pool.settings.warm_idle_seconds))
            || idle.is_some_and(|since| {
                occupancy.users.is_empty()
                    && (self.shard.draining()
                        || since.elapsed()
                            >= Duration::from_secs(self.pool.settings.linger_seconds))
            });
        drop(occupancy);
        if expire {
            self.shard.begin_drain();
        }
        expire
    }

    fn lane_open(&self, window: usize) -> bool {
        in_flight(&self.pending) < window || self.pending.len() >= REQUEST_TOMBSTONE_LIMIT
    }

    pub(super) fn drain_commands(&mut self, validated: &mut Instant) -> Result<()> {
        for _ in 0..COMMAND_DRAIN_BUDGET {
            // Cleanup Calls apply channel backpressure at the reserved RPC
            // window. Replies/inspect/release have a separate bounded lane,
            // so waiting Calls cannot hide operations that need no ACK.
            let cmd = self
                .essential
                .try_recv()
                .ok()
                .or_else(|| {
                    self.lane_open(PRIORITY_RPC_WINDOW)
                        .then(|| self.priority.try_recv().ok())
                        .flatten()
                })
                .or_else(|| {
                    self.lane_open(NORMAL_RPC_WINDOW)
                        .then(|| self.normal.try_recv().ok())
                        .flatten()
                });
            let Some(cmd) = cmd else {
                break;
            };
            self.handle_command(cmd, validated)?;
        }
        Ok(())
    }

    pub(super) fn drain_frames(&mut self) -> Result<()> {
        // Bound one drain pass so busy output cannot starve control requests.
        for _ in 0..FRAME_DRAIN_BUDGET {
            let value = match self.rpc.pending.pop_front() {
                Some(frame_value) => Some(frame_value),
                None => self.rpc.next()?,
            };
            self.publish_deltas(
                value
                    .as_ref()
                    .map(crate::runtime::provider::protocol::IncomingFrame::seq),
            );
            let Some(frame_value) = value else {
                break;
            };
            if self.rpc.uncertain() {
                return Err(protocol());
            }
            // Native Claude may exit immediately after a terminal error. Fence
            // before routing it: an interrupted result otherwise lets the worker
            // submit its next message before the following EOF is observed.
            if self.rpc.expected_claude_exit() {
                self.shard.begin_drain();
                self.pool.wake.all();
            }
            self.handle_frame(frame_value)?;
        }
        Ok(())
    }

    /// Publish deltas read before the next frame, by native thread, after
    /// resyncing viewers of threads that lost deltas. Unknown threads are dropped.
    fn publish_deltas(&mut self, before: Option<u64>) {
        let crate::runtime::provider::live_tap::Ready {
            deltas,
            gaps,
            all_gaps,
        } = self.rpc.deltas.before(before);
        let live = &self.pool.wake.live;
        if all_gaps {
            for agent in self.threads.values() {
                live.gap(agent);
            }
        }
        for thread in gaps {
            if let Some(agent) = self.threads.get(&thread) {
                live.gap(agent);
            }
        }
        for crate::runtime::provider::live_tap::Delta { thread, event } in deltas {
            if let Some(agent) = self.threads.get(&thread) {
                live.publish(agent, || Some(event));
            }
        }
    }

    pub(super) fn fail_owner(&mut self, agent: &str, error: Error) -> Result<()> {
        // Even a local delivery failure can race an unresolved resume ACK. Keep
        // this cohort closed to replacement executions until the process exits.
        if self.shard.begin_drain() {
            self.pool.wake.all();
        }
        for request in self.pending.values_mut().filter(|r| r.agent == agent) {
            request.deadline = None;
            if let Some(reply) = request.reply.take() {
                observe_reply(reply.send(Err(error.clone())), ReplyOwner::Provider);
            }
        }
        if let Some(route) = self.routes.remove(agent) {
            route.failure.send_replace(Some(error.clone()));
            let agent = agent.to_owned();
            let generation = self.shard.id.clone();
            self.pool.hub.blocking_with(move |core| {
                let mut stored_agent = core.store().agent(&agent)?;
                if stored_agent.provider_id() == Some(&generation) {
                    core.store().uncertain(&agent)?;
                    stored_agent.error = Some(error.code().into());
                    core.store().save(&stored_agent)?;
                    core.store().event(
                        &stored_agent,
                        EventKind::SessionQuarantined,
                        api::value(events::ErrorEvent { error: &error }),
                    )?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }
    pub(super) fn expire_requests(&mut self) -> Result<()> {
        let expired: Vec<_> = self
            .pending
            .values()
            .filter(|r| r.deadline.is_some_and(|d| Instant::now() >= d))
            .map(|r| r.agent.clone())
            .collect();
        for agent in expired {
            if let Some((thread, _)) = self.threads.iter().find(|(_, owner)| **owner == agent) {
                self.sink_threads.insert(thread.clone());
            }
            self.fail_owner(&agent, Error::provider_timeout(ACK_TIMEOUT))?;
            // Tombstones survive until an ACK reconciles lifecycle or this
            // process exits. Stop admission, but let siblings finish turns.
        }
        Ok(())
    }

    pub(super) fn interrupt_failed_owners(&mut self) -> Result<()> {
        // Failed owners have no event consumer. Reject their tool requests
        // below, observe their lifecycle, and interrupt any acknowledged turn.
        for thread in self.sink_threads.clone() {
            let Some(agent) = self.threads.get(&thread) else {
                self.sink_threads.remove(&thread);
                continue;
            };
            match self.rpc.active_turn(&thread) {
                Ok(Some(turn))
                    if in_flight(&self.pending) < PRIORITY_RPC_WINDOW
                        && self.pending.len() < REQUEST_TOMBSTONE_LIMIT =>
                {
                    self.sink_threads.remove(&thread);
                    if !self.interrupted.insert(InterruptedTurn {
                        thread: thread.clone(),
                        turn: turn.clone(),
                    }) {
                        continue;
                    }
                    let id = self
                        .rpc
                        .begin_request(outgoing::interrupt(&thread, &turn))?;
                    self.pending.insert(
                        id,
                        Pending {
                            agent: agent.clone(),
                            method: ClientMethod::TurnInterrupt,
                            thread: Some(thread),
                            reply: None,
                            deadline: Some(Instant::now() + REQUEST_ACK_TIMEOUT),
                        },
                    );
                }
                Ok(None) => {
                    self.sink_threads.remove(&thread);
                }
                Err(error) => {
                    // A late start ACK may identify a still-submitting turn.
                    crate::observability::observation_failed("interrupt_pending_turn", &error);
                }
                Ok(Some(_)) => {} // The reserved RPC window is temporarily full.
            }
        }
        Ok(())
    }

    pub(super) fn wait_for_input(&mut self) -> Result<()> {
        if !self.rpc.buffered()
            && (self.normal.is_empty() || !self.lane_open(NORMAL_RPC_WINDOW))
            && self.essential.is_empty()
            && (self.priority.is_empty() || !self.lane_open(PRIORITY_RPC_WINDOW))
        {
            // The no-request poll default is conditional; it is not a cap on live ACKs.
            let timeout = self
                .pending
                .values()
                .filter_map(|r| {
                    r.deadline
                        .map(|d| d.saturating_duration_since(Instant::now()))
                })
                .min()
                .unwrap_or(SHARD_IDLE_POLL_TIMEOUT);
            let timeout = self.closing.map_or(timeout, |d: Instant| {
                timeout.min(d.saturating_duration_since(Instant::now()))
            });
            let timeout = if self.shard.occupancy().slots.is_empty() {
                timeout.min(
                    Duration::from_secs(self.pool.settings.warm_idle_seconds)
                        .saturating_sub(self.shard.created.elapsed()),
                )
            } else {
                timeout
            };
            let timeout = self.shard.occupancy().idle_since.map_or(timeout, |since| {
                timeout.min(
                    Duration::from_secs(self.pool.settings.linger_seconds)
                        .saturating_sub(since.elapsed()),
                )
            });
            self.rpc.wait(Some(self.wake.as_fd()), timeout)?;
        }
        Ok(())
    }
}
