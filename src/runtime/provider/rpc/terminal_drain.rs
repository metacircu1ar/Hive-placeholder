//! Bounded observation after a local Codex interrupt; never an ownership proof.
use super::{IncomingFrame, Rpc, ServerMethod, Turn, protocol};
use crate::error::{Error, Result};
pub(crate) use crate::runtime::provider::cleanup::{LIMIT, Snapshot};
use crate::runtime::provider::error::TERMINAL_QUIET_TIMEOUT;
use std::time::Instant;

pub(super) struct Drain {
    turn: String,
    pub(super) deadline: Option<Instant>,
    allow_started: bool,
    revision: u64,
}

impl Rpc {
    /// Called before the local interrupt, including the natural-completion race.
    /// Raw process shutdown deliberately does not arm an observation window.
    pub(crate) fn arm_terminal_drain(&mut self, thread: &str, turn: &str) -> Result<()> {
        if self.claude.is_some() {
            return Ok(());
        }
        let state = self.threads.get_mut(thread).ok_or_else(protocol)?;
        if state.drained.as_deref() == Some(turn) {
            return Err(protocol());
        }
        let completed =
            matches!(state.turn, Some(Turn::Idle)) && state.completed.as_deref() == Some(turn);
        if !completed && !matches!(&state.turn, Some(Turn::Active(id)) if id == turn) {
            return Err(protocol());
        }
        let drain = state.drain.get_or_insert(Drain {
            turn: turn.into(),
            deadline: completed.then(|| Instant::now() + LIMIT),
            allow_started: true,
            revision: 0,
        });
        if drain.turn != turn {
            return Err(protocol());
        }
        // Promote only this exact epoch when completion won the dispatch race;
        // an existing fallback observation never receives a new deadline.
        drain.allow_started = true;
        Ok(())
    }

    pub(super) fn complete_drain(&mut self, thread: &str, turn: &str) {
        if let Some(drain) = self
            .threads
            .get_mut(thread)
            .and_then(|state| state.drain.as_mut())
            && drain.turn == turn
        {
            drain.deadline.get_or_insert_with(|| Instant::now() + LIMIT);
        }
    }

    pub(super) fn observe_draining_item(
        &mut self,
        frame: &IncomingFrame,
        thread: &str,
        turn: &str,
    ) -> bool {
        let Some(state) = self.threads.get_mut(thread) else {
            return false;
        };
        if state.drained.as_deref() == Some(turn)
            && !(matches!(state.turn, Some(Turn::Idle))
                && !frame.has_id()
                && (frame.method() == Some(&ServerMethod::TurnCompleted)
                    || (frame.method().is_some_and(ServerMethod::is_item)
                        && frame.method() != Some(&ServerMethod::ItemStarted))))
        {
            state.late_after_drain = true;
            self.quarantine(thread);
            return true;
        }
        if !matches!(state.turn, Some(Turn::Idle))
            || state.completed.as_deref() != Some(turn)
            || frame.has_id()
            || !frame.method().is_some_and(ServerMethod::is_item)
        {
            return false;
        }
        let Some(drain) = state.drain.as_mut() else {
            return false;
        };
        if drain.turn != turn
            || drain
                .deadline
                .is_none_or(|deadline| Instant::now() >= deadline)
            || (!drain.allow_started && frame.method() == Some(&ServerMethod::ItemStarted))
        {
            return false;
        }
        drain.revision += 1;
        true
    }

    /// Consume already-observed passive frames so stopped workers cannot fill
    /// their bounded delivery queue while the shard observes native cleanup.
    pub(crate) fn draining_item(&self, frame: &IncomingFrame) -> bool {
        !frame.has_id()
            && frame.method().is_some_and(ServerMethod::is_item)
            && frame.params().thread_id().is_some_and(|thread| {
                self.threads.get(thread).is_some_and(|state| {
                    !state.quarantined
                        && matches!(state.turn, Some(Turn::Idle))
                        && state.drain.as_ref().is_some_and(|drain| {
                            frame.params().turn_id() == Some(drain.turn.as_str())
                                && drain
                                    .deadline
                                    .is_some_and(|deadline| Instant::now() < deadline)
                        })
                })
            })
    }

    /// Compare and close in the same owner that observes native frames; only a
    /// caller that observed empty lists for QUIET may supply a revision.
    pub(crate) fn terminal_drain(
        &mut self,
        thread: &str,
        turn: &str,
        finish: Option<u64>,
    ) -> Result<Snapshot> {
        if self.claude.is_some() || !self.clean_thread(thread) {
            return Err(protocol());
        }
        let state = self.threads.get_mut(thread).ok_or_else(protocol)?;
        if state.drained.as_deref() == Some(turn) || state.completed.as_deref() != Some(turn) {
            return Err(protocol());
        }
        if finish.is_some() && state.drain.is_none() {
            return Err(protocol());
        }
        let drain = state.drain.get_or_insert(Drain {
            turn: turn.into(),
            deadline: Some(Instant::now() + LIMIT),
            allow_started: false,
            revision: 0,
        });
        if drain.turn != turn {
            return Err(protocol());
        }
        let deadline = drain.deadline.ok_or_else(protocol)?;
        let now = Instant::now();
        if now >= deadline {
            return Err(Error::provider_timeout(TERMINAL_QUIET_TIMEOUT));
        }
        let revision = drain.revision;
        let closed_at = if finish == Some(revision) {
            state.drain = None;
            state.drained = Some(turn.into());
            Some(now)
        } else {
            None
        };
        Ok(Snapshot {
            deadline,
            revision,
            closed_at,
        })
    }

    pub(crate) fn thread_failure(&self, thread: &str) -> Error {
        if self
            .threads
            .get(thread)
            .is_some_and(|state| state.late_after_drain)
        {
            Error::late_activity_after_drain(
                "Provider activity resumed after bounded terminal cleanup.",
            )
        } else {
            protocol()
        }
    }
}
