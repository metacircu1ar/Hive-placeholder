//! Fence shard commands and settle process ownership during shutdown.
use super::{Cmd, Error, EventKind, MutexExt, ProviderEventKind, Result, ShardLoop, unavailable};
use crate::observability::{ReplyOwner, reply as observe_reply};
use crate::runtime::api::{self, events};
use crate::state::ProviderEvidence;
use std::mem;

impl ShardLoop<'_> {
    pub(super) fn fence_commands(&mut self) {
        // No more commands can reach native I/O after the loop exits. Fence idle
        // workers before publishing final events, and return exact unsent proof
        // for commands that raced the fence instead of dropping their replies.
        self.shard.begin_drain();
        self.pool.wake.all();
        self.normal.close();
        self.priority.close();
        self.essential.close();
        for queue in [&mut *self.normal, &mut *self.priority, &mut *self.essential] {
            while let Some(command) = queue.blocking_recv() {
                match command {
                    Cmd::Call(_, request, reply) => {
                        let method = request.method();
                        let error = method
                            .submission_error(self.shard.backend.multiplexed(), unavailable());
                        observe_reply(reply.send(Err(error)), ReplyOwner::Provider);
                    }
                    Cmd::Release { reply, .. } => self.release_replies.push(reply),
                    // These requests cannot submit input. Dropping their reply
                    // reports an unavailable provider without inventing success.
                    _ => {}
                }
            }
        }
    }

    pub(super) fn shutdown(&mut self, evidence: &mut ProviderEvidence, result: Result<()>) {
        self.fence_commands();
        let error = result.err();
        if let Some(error) = &error
            && !(matches!(error, Error::ProviderExited { .. }) && self.rpc.expected_claude_exit())
        {
            tracing::error!(name: "provider.execution_failed",
                 provider_id = self.shard.id.as_str(),
                 code = error.code(),
                 error = error as &(dyn std::error::Error + 'static),
                 decision = "close_provider");
        }
        self.poison_shutdown_error(error.as_ref());
        // Quiesce every still-owned thread before closing the shared process.
        let mut report = self.rpc.close();
        if report.reaped {
            self.shard.release_capacity();
        }
        if let Err(cause) = evidence.finish(report.quiescent, report.confirmed) {
            tracing::error!(name: "provider.receipt_finish_failed",
                 provider_id = self.shard.id.as_str(),
                 code = cause.code(),
                 error = &cause as &(dyn std::error::Error + 'static),
                 decision = "quarantine");
            report.confirmed = false;
        }
        if !report.confirmed {
            self.pool.quarantine(self.shard);
        }
        let failed_provider = self.shard.id.clone();
        for (_, request) in mem::take(&mut self.pending) {
            if let Some(reply) = request.reply {
                observe_reply(
                    reply.send(Err(error.clone().unwrap_or_else(unavailable))),
                    ReplyOwner::Provider,
                );
            }
        }
        // Publish before closing watch senders: validated final observations must
        // remain readable when native error-result EOF wins the receiver race.
        self.shard
            .publish_final_events(report.confirmed && self.rpc.expected_claude_exit());
        self.routes.clear();
        self.record_shutdown(error.as_ref(), &report, failed_provider);
        self.publish_release_replies(&report);
        self.pool.publish_exit(self.shard);
    }
    fn poison_shutdown_error(&mut self, error: Option<&Error>) {
        if error.is_some_and(|e| {
            if matches!(e, Error::ProviderExited { .. }) && self.rpc.expected_claude_exit() {
                return false;
            }
            !matches!(
                e,
                Error::ProviderTimeout { .. }
                    | Error::AuthenticationRequired { .. }
                    | Error::ProviderRequestFailed { .. }
            )
        }) {
            self.rpc.poison();
        }
    }
    fn record_shutdown(
        &self,
        error: Option<&Error>,
        report: &super::Shutdown,
        failed_provider: String,
    ) {
        let slots: Vec<_> = self.shard.occupancy().slots.iter().cloned().collect();
        let generation = self.shard.id.clone();
        let saved = report.clone();
        let failure = error.map(Error::code);
        let recorded = self.pool.hub.blocking_with(move |core| {
            if !saved.confirmed {
                core.quarantine_provider(&failed_provider)?;
            }
            core.store().provider_event(
                &generation,
                ProviderEventKind::Shutdown,
                api::value(events::ProcessShutdown {
                    shutdown: &saved,
                    error_code: failure,
                }),
            )?;
            for id in slots {
                // Released sessions remain in historical slots until process exit.
                // Purge never releases their physical capacity or shared evidence.
                let Some(stored_agent) = core.store().agent_or_purged(&id)? else {
                    continue;
                };
                if stored_agent.provider_id() == Some(&generation) {
                    core.store().event(
                        &stored_agent,
                        EventKind::ProviderShutdown,
                        api::value(&saved),
                    )?;
                }
            }
            Ok(())
        });
        if let Err(secondary) = recorded {
            self.pool.hub.stop_after_failure(&secondary);
            if let Some(primary) = error {
                tracing::error!(name: "provider.shutdown_record_failed",
                     provider_id = self.shard.id.as_str(),
                     code = secondary.code(),
                     primary = primary as &(dyn std::error::Error + 'static),
                     secondary = &secondary as &(dyn std::error::Error + 'static),
                     sqlite_code = crate::runtime::store::sqlite_code(&secondary),
                     decision = "shutdown_retain_evidence");
            } else {
                tracing::error!(name: "provider.shutdown_record_failed",
                     provider_id = self.shard.id.as_str(),
                     code = secondary.code(),
                     error = &secondary as &(dyn std::error::Error + 'static),
                     sqlite_code = crate::runtime::store::sqlite_code(&secondary),
                     decision = "shutdown_retain_evidence");
            }
        }
    }
    fn publish_release_replies(&mut self, report: &super::Shutdown) {
        *self.shard.report.lock_or_panic() = Some(report.clone());
        for tx in mem::take(&mut self.release_replies) {
            observe_reply(tx.send(report.clone()), ReplyOwner::Provider);
        }
    }
}
