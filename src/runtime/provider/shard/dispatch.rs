//! Command and frame routing within an owned provider shard.
mod limits;

use super::super::protocol::{ClientMethod, IncomingFrame, Response, ServerMethod, outgoing};
use super::{
    ACCOUNT_REVALIDATION_AGE, Cmd, Pending, REQUEST_ACK_TIMEOUT, REQUEST_TOMBSTONE_LIMIT,
    SHARD_CLOSE_GRACE, ShardLoop, Shutdown, protocol, thread_key, unavailable, validate_account,
};
use crate::observability::{ReplyOwner, reply as observe_reply};
use crate::runtime::api::{self, events};
use crate::runtime::auth_fallback::AuthReason;
use crate::runtime::auth_fallback::signals;
use crate::runtime::branching::BranchPhase;
use crate::runtime::domain::AuthCutoff;
use crate::runtime::provider::ShutdownMethod;
use crate::state;
use crate::{
    error::{Error, Result},
    runtime::domain::{EventKind, ProviderEventKind},
};
use serde_json::Value;
use std::{sync::atomic::Ordering, time::Instant};
use tokio::sync::oneshot;

pub(super) struct Cutoff {
    agent: String,
    turn: String,
}
impl ShardLoop<'_> {
    pub(super) fn handle_command(&mut self, cmd: Cmd, validated: &mut Instant) -> Result<()> {
        match cmd {
            Cmd::Attach(agent, route, tx) => {
                self.attach_route(agent, route, tx, validated)?;
            }
            Cmd::Call(agent, request, tx) => {
                self.handle_call(agent, request, tx)?;
            }
            Cmd::Send(value, tx) => {
                let result = self.rpc.send(value);
                observe_reply(tx.send(result.clone()), ReplyOwner::Provider);
                result?;
            }
            #[cfg(feature = "comb")]
            Cmd::ForgetCombTurn(turn, tx) => {
                self.rpc.forget_comb_turn(&turn);
                observe_reply(tx.send(()), ReplyOwner::Provider);
            }
            #[cfg(feature = "comb")]
            Cmd::ForgetTool(id, tx) => {
                observe_reply(tx.send(self.rpc.forget_tool(&id)), ReplyOwner::Provider);
            }
            Cmd::CommitBranch(turn, tx) => {
                let result = self.rpc.commit_branch(&turn);
                observe_reply(tx.send(result.clone()), ReplyOwner::Provider);
                result?;
            }
            Cmd::Inspect(thread, tx) => {
                observe_reply(tx.send(self.rpc.active_turn(&thread)), ReplyOwner::Provider);
            }
            Cmd::TerminalDrain(thread, turn, finish, tx) => {
                observe_reply(
                    tx.send(self.rpc.terminal_drain(&thread, &turn, finish)),
                    ReplyOwner::Provider,
                );
            }
            Cmd::Retire(thread, tx) => {
                observe_reply(
                    tx.send(self.rpc.retire_thread(&thread)),
                    ReplyOwner::Provider,
                );
            }
            Cmd::Quarantine(thread, tx) => {
                self.rpc.quarantine(&thread);
                observe_reply(tx.send(()), ReplyOwner::Provider);
            }
            Cmd::Release {
                agent,
                last,
                clean,
                reply: tx,
            } => {
                self.routes.remove(&agent);
                if !clean || last {
                    // An unresolved owner waits for process cleanup;
                    // never claim early release or poison healthy routes.
                    self.release_replies.push(tx);
                    if last {
                        self.closing = Some(Instant::now() + SHARD_CLOSE_GRACE);
                    }
                    return Ok(());
                }
                observe_reply(
                    tx.send(Shutdown {
                        method: ShutdownMethod::SessionReleased,
                        reaped: false,
                        confirmed: clean,
                        quiescent: clean,
                        exit_code: None,
                    }),
                    ReplyOwner::Provider,
                );
            }
        }
        Ok(())
    }

    fn attach_route(
        &mut self,
        agent: String,
        route: super::Route,
        tx: oneshot::Sender<Result<super::Started>>,
        validated: &mut Instant,
    ) -> Result<()> {
        if route.attach_cancelled.load(Ordering::Acquire) {
            return Ok(());
        }
        // A retired user's Release may be queued after this Attach.
        // Consume the empty-admission obligation even when still fresh;
        // a later Attach with active peers cannot run synchronous RPCs.
        let empty = self.shard.take_empty_attach_check();
        if (empty || self.routes.is_empty()) && validated.elapsed() >= ACCOUNT_REVALIDATION_AGE {
            validate_account(self.rpc)?;
            self.shard
                .lease
                .verify_account(self.rpc.account_identity.as_deref())?;
            *validated = Instant::now();
        }
        self.routes.insert(agent, route);
        observe_reply(tx.send(Ok(self.started.clone())), ReplyOwner::Provider);
        Ok(())
    }
    fn handle_call(
        &mut self,
        agent: String,
        request: outgoing::OutgoingRequest,
        tx: oneshot::Sender<Result<Response>>,
    ) -> Result<()> {
        let method = request.method();
        if !self.routes.contains_key(&agent) && !method.is_cleanup() {
            // A command may have entered the queue immediately
            // before its owner's independent failure signal.
            let error = unavailable();
            observe_reply(
                tx.send(Err(
                    method.submission_error(self.shard.backend.multiplexed(), error)
                )),
                ReplyOwner::Provider,
            );
            return Ok(());
        }
        if self.pending.len() >= REQUEST_TOMBSTONE_LIMIT {
            let error = Error::provider_timeout(
                "Provider request tombstones are full; cohort cleanup is required.",
            );
            observe_reply(
                tx.send(Err(
                    method.submission_error(self.shard.backend.multiplexed(), error)
                )),
                ReplyOwner::Provider,
            );
            self.shard.begin_drain();
            self.pool.wake.all();
            return Ok(());
        }
        if self.shard.draining() && method.submits_input() {
            let error =
                Error::provider_draining("Provider cohort is draining; input was not submitted.");
            observe_reply(
                tx.send(Err(if method == ClientMethod::ThreadFork {
                    error.before_submission()
                } else {
                    error
                })),
                ReplyOwner::Provider,
            );
            return Ok(());
        }
        self.submit_call(agent, request, tx)
    }

    fn submit_call(
        &mut self,
        agent: String,
        request: outgoing::OutgoingRequest,
        tx: oneshot::Sender<Result<Response>>,
    ) -> Result<()> {
        let method = request.method();
        // A fork's request ID names the source, which is never this route's owned thread.
        let thread = if method == ClientMethod::ThreadFork {
            None
        } else {
            request.thread().map(str::to_owned)
        };
        if method == ClientMethod::TurnInterrupt
            && let (Some(thread), Some(turn)) = (thread.as_deref(), request.interrupted_turn())
            && let Err(error) = self.rpc.arm_terminal_drain(thread, turn)
        {
            observe_reply(tx.send(Err(error)), ReplyOwner::Provider);
            return Ok(());
        }
        let id = match self.rpc.begin_request(request) {
            Ok(id) => id,
            Err(e) => {
                let fatal = self.rpc.uncertain();
                observe_reply(
                    tx.send(Err(
                        if method == ClientMethod::TurnStart && self.rpc.claude_before_input() {
                            e.before_submission()
                        } else {
                            e
                        },
                    )),
                    ReplyOwner::Provider,
                );
                if fatal {
                    return Err(protocol());
                }
                return Ok(());
            }
        };
        self.pending.insert(
            id,
            Pending {
                agent,
                method,
                thread,
                reply: Some(tx),
                deadline: if method == ClientMethod::TurnSteer && !self.shard.backend.multiplexed()
                {
                    None
                } else {
                    Some(Instant::now() + REQUEST_ACK_TIMEOUT)
                },
            },
        );
        Ok(())
    }

    // Consume each routed frame here so its allocation is released before the next event.
    fn handle_response(&mut self, frame_value: IncomingFrame) -> Result<()> {
        let id = frame_value.id().ok_or_else(protocol)?;
        let request = self.pending.remove(&id).ok_or_else(protocol)?;
        let result = self.rpc.finish_response(&frame_value);
        self.bind_thread(&request, &result)?;
        if request.method == ClientMethod::TurnStart
            && result.is_err()
            && request
                .reply
                .as_ref()
                .is_some_and(|reply| !reply.is_closed())
            && let Some(signal) = signals::codex_error(frame_value.error_data())
        {
            self.record_limit(signal, None)?;
        }
        let owner_uncertain = request
            .thread
            .as_deref()
            .is_some_and(|t| self.rpc.thread_uncertain(t));
        let malformed = result.as_ref().is_err_and(|e| {
            !(matches!(e, Error::ProviderRequestFailed { .. })
                || (request.method == ClientMethod::TurnSteer
                    && matches!(
                        e,
                        Error::NoActiveTurn { .. }
                            | Error::StaleTurn { .. }
                            | Error::UnsupportedOperation { .. }
                            | Error::SteeringUncertain { .. }
                    )))
        });
        if self.rpc.uncertain() {
            return Err(protocol());
        }
        if owner_uncertain || malformed {
            if let Some(thread) = &request.thread {
                self.sink_threads.insert(thread.clone());
            }
            self.fail_owner(&request.agent, protocol())?;
        }
        if let Some(reply) = request.reply {
            observe_reply(
                reply.send(if owner_uncertain {
                    Err(protocol())
                } else {
                    result
                }),
                ReplyOwner::Provider,
            );
        }
        Ok(())
    }

    fn bind_thread(&mut self, request: &Pending, result: &Result<Response>) -> Result<()> {
        if request.method.opens_thread()
            && let Ok(data) = result
        {
            let thread = data.thread_id().ok_or_else(protocol)?;
            if self
                .threads
                .get(thread)
                .is_some_and(|owner| owner != &request.agent)
                || self
                    .threads
                    .iter()
                    .any(|(existing, owner)| owner == &request.agent && existing != thread)
            {
                self.rpc.poison();
                return Err(protocol());
            }
            self.threads.insert(thread.into(), request.agent.clone());
            if request.reply.is_none() || request.method == ClientMethod::ThreadFork {
                let late = request.reply.is_none();
                if late {
                    self.sink_threads.insert(thread.to_owned());
                }
                self.record_bound_thread(request, thread, late)?;
            }
        }
        Ok(())
    }

    fn record_bound_thread(&self, request: &Pending, thread: &str, late: bool) -> Result<()> {
        let agent = request.agent.clone();
        let native = thread.to_owned();
        let generation = self.shard.id.clone();
        let fork = request.method == ClientMethod::ThreadFork;
        self.pool.hub.blocking_with(move |core| {
            // Native thread tracking above survives deletion of a
            // safely released historical owner, including late ACKs.
            let Some(mut stored_agent) = core.store().agent_or_purged(&agent)? else {
                return Ok(());
            };
            if stored_agent.provider_id() == Some(&generation) && stored_agent.thread_id.is_none() {
                stored_agent.thread_id = Some(native.clone());
                core.store().save(&stored_agent)?;
                core.store().event(
                    &stored_agent,
                    EventKind::ThreadBound,
                    api::value(events::ThreadBound {
                        thread_id: &native,
                        late_ack: Some(late),
                    }),
                )?;
                if fork {
                    core.store().record_branch_native(&stored_agent, &native)?;
                    // Reconcile the exact returned identity; an abandoned branch stays fenced.
                    if late
                        && stored_agent.branch_phase == Some(BranchPhase::Forking)
                        && !stored_agent.terminal()
                        && !stored_agent.paused
                        && stored_agent.control.is_none()
                        && core
                            .store()
                            .branch_intent(&agent)?
                            .and_then(|b| b.deadline)
                            .is_some_and(|d| d > state::now())
                    {
                        core.store().bind_branch(&mut stored_agent, &native)?;
                    }
                }
            }
            Ok(())
        })?;
        Ok(())
    }

    // Consume each routed frame here so its allocation is released before the next event.
    #[expect(
        clippy::expect_used,
        reason = "dispatch_frame selects an owner only from a present thread key; moving \
            the frame cannot erase the retained key."
    )]
    fn handle_owner_frame(&mut self, agent: String, frame_value: IncomingFrame) -> Result<()> {
        // Watched agents get a live copy; routing below is unchanged.
        self.pool.wake.live.publish(&agent, || {
            crate::runtime::provider::live_tap::event(&frame_value)
        });
        if let Some(thread) = thread_key(&frame_value).filter(|t| self.rpc.thread_uncertain(t)) {
            self.sink_threads.insert(thread.to_owned());
            self.fail_owner(&agent, self.rpc.thread_failure(thread))?;
        }
        if self.rpc.draining_item(&frame_value) {
            return Ok(());
        }
        // Retain only the routing metadata needed after the queue owns the payload.
        let thread = thread_key(&frame_value).map(str::to_owned);
        let request_id = frame_value.has_id().then(|| frame_value.raw_id().clone());
        let delivery = self
            .routes
            .get(&agent)
            .map(|route| route.send(frame_value, &self.bytes));
        if let Some(Err(error)) = delivery {
            self.sink_threads
                .insert(thread.expect("owned frames have a thread key"));
            self.fail_owner(&agent, error)?;
        }
        if self
            .routes
            .get(&agent)
            .is_none_or(|route| route.attach_cancelled.load(Ordering::Acquire))
            && let Some(request_id) = request_id
        {
            // Failed owners become an unbuffered sink. Lifecycle is
            // still observed by Rpc; no more agent tools execute.
            self.rpc.send(outgoing::unsupported(
                &request_id,
                "Session is not accepting tool requests.",
            ))?;
        }
        Ok(())
    }

    // Consume each routed frame here so its allocation is released before the next event.
    fn handle_unowned_request(&mut self, frame_value: IncomingFrame) -> Result<()> {
        // Account/attestation requests without an owned thread
        // are answered at shard scope, never charged to a random agent.
        self.rpc.send(outgoing::unsupported(
            frame_value.raw_id(),
            "Unsupported provider request.",
        ))?;
        let generation = self.shard.id.clone();
        self.pool.hub.blocking_with(move |core| {
            core.store().provider_event(
                &generation,
                ProviderEventKind::UnsupportedRequest,
                api::value(events::ProviderNotice {
                    scope: "unattributed",
                    details: None,
                }),
            )
        })?;
        Ok(())
    }

    // Consume each routed frame here so its allocation is released before the next event.
    fn handle_notice(&mut self, frame_value: IncomingFrame) -> Result<()> {
        let kind = match frame_value.method() {
            Some(ServerMethod::AccountUpdated) => ProviderEventKind::AccountUpdated,
            Some(ServerMethod::RateLimitsUpdated) => ProviderEventKind::RateLimitsUpdated,
            Some(ServerMethod::ConfigurationWarning) => ProviderEventKind::ConfigurationWarning,
            Some(ServerMethod::Error) => ProviderEventKind::ProviderError,
            Some(
                ServerMethod::ThreadStarted
                | ServerMethod::ThreadStatusChanged
                | ServerMethod::ThreadClosed,
            ) => ProviderEventKind::ThreadMetadata,
            _ => ProviderEventKind::ConnectionNotification,
        };
        if self.notices.insert(kind.as_str().into()) {
            let generation = self.shard.id.clone();
            self.pool.hub.blocking_with(move |core| {
                core.store().provider_event(
                    &generation,
                    kind,
                    api::value(events::ProviderNotice {
                        scope: "connection",
                        details: Some("redacted"),
                    }),
                )
            })?;
        }
        Ok(())
    }

    pub(super) fn handle_frame(&mut self, frame_value: IncomingFrame) -> Result<()> {
        self.observe_limits(&frame_value)?;
        if frame_value.is_response() {
            self.handle_response(frame_value)
        } else if let Some(agent) = thread_key(&frame_value)
            .and_then(|thread| self.threads.get(thread))
            .cloned()
        {
            self.handle_owner_frame(agent, frame_value)
        } else if frame_value.has_id() {
            self.handle_unowned_request(frame_value)
        } else {
            // thread/started may precede its ACK. Ownership is established from
            // that response; other unattributed notifications are connection metadata.
            self.handle_notice(frame_value)
        }
    }
}
