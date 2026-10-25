//! Limits for this component.
use super::ShardLoop;
use super::{AuthCutoff, AuthReason, Cutoff, IncomingFrame, Result, ServerMethod, Value, signals};

impl ShardLoop<'_> {
    pub(in crate::runtime::provider::shard) fn record_limit(
        &mut self,
        mut signal: signals::Signal,
        cutoff: Option<Cutoff>,
    ) -> Result<()> {
        let auth = self.shard.auth.clone();
        let generation = self.shard.lease.generation();
        let provider = self.shard.id.clone();
        signal.key.clone_from(&self.shard.key);
        if signal.key.is_some() {
            // API method selection alone cannot justify periodic quota probes.
            if signal.reason == crate::runtime::auth_fallback::AuthReason::Authentication
                || (signal.state == crate::runtime::auth_fallback::AvailabilityState::Limited
                    && signal.until.is_none())
            {
                signal.state = crate::runtime::auth_fallback::AvailabilityState::Unavailable;
            }
        }
        self.pool.hub.blocking_with(move |core| {
            core.auth_signal(&auth, generation, &provider, signal)?;
            if let Some(Cutoff { agent: id, turn }) = cutoff {
                let mut agent = core.store().agent(&id)?;
                if agent.auth_generation == generation
                    && agent.provider_id() == Some(&provider)
                    && (agent.turn_id.as_deref() == Some(&turn)
                        || (agent.turn_id.is_none() && agent.turn_selection.is_some()))
                {
                    let message = if agent.turn_id.as_deref() == Some(&turn) {
                        core.store().cutoff_input(&agent, &turn)?
                    } else {
                        None
                    };
                    agent.auth_cutoff = Some(match message {
                        Some(message) => AuthCutoff::Bound { turn, message },
                        None => AuthCutoff::Observed { turn },
                    });
                    core.store().save(&agent)?;
                }
            }
            Ok(())
        })
    }

    pub(super) fn observe_limits(&mut self, frame: &IncomingFrame) -> Result<()> {
        let mut cutoff = false;
        let signal = match frame.method() {
            Some(ServerMethod::RateLimitsUpdated) => {
                self.pool.usage.codex(
                    &self.shard.auth,
                    self.shard.lease.generation(),
                    &frame.params().raw()["rateLimits"],
                    crate::state::now(),
                );
                if let Ok(envelope) =
                    serde_json::from_value::<signals::CodexEnvelope>(frame.params().raw().clone())
                        .inspect_err(|error| {
                            super::super::super::protocol::lenient::drift(
                                "codex_rate_limits",
                                Some(error),
                            );
                        })
                {
                    self.limits.merge(envelope.rate_limits);
                    self.limits.signal()
                } else {
                    None
                }
            }
            Some(ServerMethod::ClaudeRateLimits) => {
                self.pool.usage.claude(
                    &self.shard.auth,
                    self.shard.lease.generation(),
                    &frame.params().raw()["info"],
                    crate::state::now(),
                );
                serde_json::from_value::<signals::ClaudeLimits>(
                    frame.params().raw()["info"].clone(),
                )
                .inspect_err(|error| {
                    super::super::super::protocol::lenient::drift(
                        "claude_rate_limits",
                        Some(error),
                    );
                })
                .ok()
                .and_then(|v| v.signal())
            }
            Some(ServerMethod::Error) if frame.params().will_retry() == &Value::Bool(false) => {
                let value = signals::codex_error(&frame.params().raw()["error"]);
                cutoff = value.is_some();
                value.map(|mut signal| {
                    if signal.reason == AuthReason::UsageLimit
                        && signal.until.is_none()
                        && let Some(known) = self.limits.signal()
                    {
                        signal.until = known.until;
                        signal.reset_floor = known.reset_floor;
                        signal.rejected_resets = known.rejected_resets;
                        signal.window = known.window;
                    }
                    signal
                })
            }
            _ => None,
        };
        if let Some(signal) = signal {
            let target = cutoff
                .then(|| {
                    frame
                        .params()
                        .thread_key()
                        .and_then(|t| self.threads.get(t))
                        .zip(frame.params().turn_id())
                        .map(|(id, turn)| Cutoff {
                            agent: id.clone(),
                            turn: turn.to_owned(),
                        })
                })
                .flatten();
            self.record_limit(signal, target)?;
        }
        Ok(())
    }
}
