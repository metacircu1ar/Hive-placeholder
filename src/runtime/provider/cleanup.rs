//! Bound terminal cleanup with revision-checked observations and a fixed quiet window.
use super::{
    Session,
    error::{protocol, unavailable},
    shard::Cmd,
};
use crate::runtime::provider::error::TERMINAL_QUIET_TIMEOUT;
use crate::{
    error::{Error, Result},
    runtime::provider::protocol::outgoing,
};
use serde::Serialize;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

// Require a sustained empty inventory because native process starts have no terminal seal.
pub(crate) const QUIET: Duration = Duration::from_secs(3);
// Bound the entire cleanup observation, including repeated cleanup and command admission.
pub(crate) const LIMIT: Duration = Duration::from_secs(20);
// Recheck quiet progress without a busy loop or delaying ordinary cleanup.
const OBSERVATION_POLL: Duration = Duration::from_millis(100);
const _: () = assert!(OBSERVATION_POLL.as_millis() < QUIET.as_millis());
const _: () = assert!(QUIET.as_millis() < LIMIT.as_millis());

#[derive(Clone, Copy, Debug)]
pub(crate) struct Snapshot {
    pub deadline: Instant,
    pub revision: u64,
    pub closed_at: Option<Instant>,
}

#[derive(Serialize)]
pub(crate) struct Observation {
    kind: &'static str,
    quiet_ms: u128,
    recleans: u64,
    elapsed_ms: u128,
    deadline_ms: u128,
    late_items_tolerated: u64,
}

impl Session {
    async fn drain_snapshot(
        &self,
        thread: &str,
        turn: &str,
        finish: Option<u64>,
    ) -> Result<Snapshot> {
        let (tx, rx) = oneshot::channel();
        self.shard
            .command(
                Cmd::TerminalDrain(thread.into(), turn.into(), finish, tx),
                true,
            )
            .await?;
        rx.await.map_err(|cause| unavailable().with_source(cause))?
    }

    /// Observe three quiet seconds after empty inventory, within one fixed
    /// twenty-second window. Codex exposes no seal against future process starts.
    pub(crate) async fn observe_terminal_cleanup(
        &self,
        thread: &str,
        turn: &str,
    ) -> Result<Observation> {
        // This command exchange itself can wait behind shard work. Bound it
        // too; the returned native-observation deadline is never extended.
        let entry_deadline = tokio::time::Instant::now() + LIMIT;
        let snapshot =
            tokio::time::timeout_at(entry_deadline, self.drain_snapshot(thread, turn, None))
                .await
                .map_err(|cause| {
                    Error::provider_timeout(TERMINAL_QUIET_TIMEOUT).with_source(cause)
                })??;
        let started = snapshot.deadline.checked_sub(LIMIT).ok_or_else(protocol)?;
        let deadline = entry_deadline.min(tokio::time::Instant::from_std(snapshot.deadline));
        tokio::time::timeout_at(
            deadline,
            self.observe_quiet(thread, turn, snapshot, started),
        )
        .await
        .map_err(|cause| Error::provider_timeout(TERMINAL_QUIET_TIMEOUT).with_source(cause))?
    }
    async fn observe_quiet(
        &self,
        thread: &str,
        turn: &str,
        snapshot: Snapshot,
        started: Instant,
    ) -> Result<Observation> {
        let mut snapshot = snapshot;
        let mut cleaned = None;
        let mut quiet_since: Option<Instant> = None;
        let mut cleans = 0;
        loop {
            if cleaned != Some(snapshot.revision) {
                self.call(outgoing::terminal_clean(thread)).await?;
                cleans += 1;
                cleaned = Some(snapshot.revision);
                quiet_since = None;
            }
            let inventory = self.call(outgoing::terminal_list(thread)).await?;
            let empty = inventory.terminals_empty()?;
            snapshot = self.drain_snapshot(thread, turn, None).await?;
            if empty && cleaned == Some(snapshot.revision) {
                let quiet = quiet_since.get_or_insert_with(Instant::now);
                if quiet.elapsed() >= QUIET {
                    snapshot = self
                        .drain_snapshot(thread, turn, Some(snapshot.revision))
                        .await?;
                    if let Some(closed_at) = snapshot.closed_at {
                        return Ok(Observation {
                            kind: "observed_quiet",
                            quiet_ms: closed_at.duration_since(*quiet).as_millis(),
                            recleans: cleans - 1,
                            elapsed_ms: closed_at.duration_since(started).as_millis(),
                            deadline_ms: LIMIT.as_millis(),
                            late_items_tolerated: snapshot.revision,
                        });
                    }
                }
            } else {
                quiet_since = None;
                if !empty {
                    cleaned = None;
                }
            }
            tokio::time::sleep(OBSERVATION_POLL).await;
        }
    }
}
