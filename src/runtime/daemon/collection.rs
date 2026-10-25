//! Collect completed tasks before actor-capacity decisions, with bounded work.
use super::Supervisor;
use crate::error::Result;
use std::sync::atomic::Ordering;

// Shared by both opportunistic harvests in one supervisor iteration. The normal
// event selection may then collect one more task before the next iteration.
pub(super) const BATCH_LIMIT: usize = 64;
type Finished = std::result::Result<String, tokio::task::JoinError>;

impl Supervisor<'_> {
    pub(super) async fn collect_finished(&mut self, remaining: &mut usize) -> Result<()> {
        let mut released = Vec::new();
        while *remaining > 0 && !self.stopping() {
            let Some(done) = self.workers.try_join_next() else {
                break;
            };
            *remaining -= 1;
            if let Some(id) = self.collect_result(done) {
                released.push(id);
            }
        }
        // Even if shutdown or a JoinError stopped harvesting, already collected
        // successes must finish their claim release. Never cancel this flush.
        self.release_workers(released).await
    }

    pub(super) async fn worker_finished(&mut self, done: Option<Finished>) -> Result<()> {
        let released = done.and_then(|done| self.collect_result(done));
        self.release_workers(released.into_iter().collect()).await
    }

    fn collect_result(&mut self, done: Finished) -> Option<String> {
        match done {
            Ok(id) => {
                self.occupied.remove(&id);
                self.wake.remove(&id);
                Some(id)
            }
            Err(error) => {
                let code = if error.is_panic() {
                    "panicked"
                } else {
                    "cancelled"
                };
                let task_id = error.id().to_string();
                tracing::error!(name: "worker.join_failed",
                     task_id = task_id,
                     code = code,
                     error = &error as &(dyn std::error::Error + 'static),
                     decision = "shutdown_retain_evidence");
                self.cleanup_uncertain.store(true, Ordering::Relaxed);
                None
            }
        }
    }

    async fn release_workers(&self, ids: Vec<String>) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        // Await the hub's commit acknowledgement before admission or eviction.
        // SQL rollback does not restore Core.running: an error is fatal to the
        // supervisor, whose shutdown/recovery retains unreleased claims safely.
        self.shared
            .with(move |core| {
                for id in ids {
                    core.stopped(&id);
                    core.store().release_claim(&id)?;
                    let agent = core.store().agent(&id)?;
                    if agent.waiting_reason.is_some() || agent.auth_probe_id.is_some() {
                        core.store().deadlines_changed();
                    }
                }
                Ok(())
            })
            .await
    }
}

/// The future owns its agent identity even while a panic unwinds its execution.
pub(super) struct WorkerBoundary<'a> {
    agent: &'a str,
    finished: bool,
}
impl<'a> WorkerBoundary<'a> {
    pub(super) fn new(agent: &'a str) -> Self {
        Self {
            agent,
            finished: false,
        }
    }
    pub(super) fn finish(mut self) {
        self.finished = true;
    }
}
impl Drop for WorkerBoundary<'_> {
    fn drop(&mut self) {
        if !self.finished {
            if std::thread::panicking() {
                tracing::error!(name: "worker.panicked",
                     agent_id = self.agent,
                     decision = "retain_evidence");
            } else {
                tracing::warn!(name: "worker.cancelled",
                     agent_id = self.agent,
                     decision = "retain_evidence");
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Test fixtures fail immediately on setup/assertion errors.
mod tests;
