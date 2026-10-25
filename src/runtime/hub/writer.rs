//! Execute committed writer batches, publish their wakes, and acknowledge before checkpoint
//! I/O.
use super::timing::Batch;
use super::{
    CHECKPOINT_INTERVAL, COMMIT_JOB_LIMIT, COMMIT_TIME_BUDGET, Completion, Core, Job, Wake,
};
use crate::{
    error::Result,
    observability::timing::Timer,
    runtime::{
        reader::Published,
        store::{Checkpointer, wal},
    },
};
use std::{
    sync::{Arc, atomic::Ordering},
    time::Instant,
};
use tokio::sync::mpsc;

pub(super) struct WriterLoop {
    // Fields drop in declaration order. The checkpointer's connection must close
    // before the writer's, also when a panic unwinds this loop, so that the
    // writer's close-time checkpoint can empty the WAL.
    checkpointer: Option<Checkpointer>,
    core: Core,
    receiver: mpsc::Receiver<Job>,
    observation: Arc<Published>,
    notify: Arc<Wake>,
    checkpoint_at: Instant,
    observation_sequence: u64,
}
impl WriterLoop {
    pub fn new(
        core: Core,
        receiver: mpsc::Receiver<Job>,
        observation: Arc<Published>,
        notify: Arc<Wake>,
    ) -> Self {
        let checkpointer = match core.store().defer_checkpoints() {
            Ok(()) => match core.store().start_checkpointer() {
                Ok(checkpointer) => Some(checkpointer),
                Err(error) => {
                    tracing::warn!(name: "writer.checkpointer_unavailable",
                         code = error.code(),
                         error = &error as &(dyn std::error::Error + 'static),
                         sqlite_code = super::super::store::sqlite_code(&error),
                         decision = "checkpoint_on_writer");
                    None
                }
            },
            Err(error) => {
                tracing::warn!(name: "writer.deferred_checkpoint_unavailable",
                     code = error.code(),
                     error = &error as &(dyn std::error::Error + 'static),
                     sqlite_code = super::super::store::sqlite_code(&error),
                     decision = "retain_native_policy");
                None
            }
        };
        Self {
            core,
            receiver,
            observation,
            notify,
            checkpointer,
            checkpoint_at: Instant::now(),
            observation_sequence: 0,
        }
    }
    pub fn run(mut self) -> Result<()> {
        let result = loop {
            let first = self.receiver.blocking_recv();
            let Some(first) = first else {
                break Ok(());
            };
            if let Err(error) = self.batch(first) {
                break Err(error);
            }
        };
        // The writer's connection closes last, so SQLite's close-time checkpoint
        // still empties the WAL.
        if let Some(checkpointer) = self.checkpointer.take() {
            checkpointer.stop();
        }
        drop(self.core);
        result
    }
    fn batch(&mut self, first: Job) -> Result<()> {
        // This state machine keeps transaction, publication, reply and checkpoint effects ordered.
        let mut timing = Batch::start();
        // Start the first request immediately; batching never preempts a running job.
        if let Err(error) = self.core.store().begin() {
            self.failed("begin", &error);
            timing.finish("begin_failed", false);
            return Err(error);
        }
        let apply = Timer::start();
        let replies = self.apply(first, &mut timing);
        let committed = self.commit(&mut timing, apply);
        if let Err(error) = &committed {
            self.failed("commit", error);
        }
        self.publish_wakes(&committed);
        timing.frames = wal::frames();
        // Decide before acknowledgements can enqueue follow-up work.
        let checkpoint = committed.is_ok().then(|| self.checkpoint_due()).flatten();
        // The batch is durable: checkpoint I/O must not hold its replies.
        for reply in replies {
            reply(committed.clone());
        }
        if let Some(kind) = checkpoint {
            let timer = Timer::start();
            self.checkpoint(kind);
            timing.checkpoint_ns = timer.elapsed_ns();
            timing.checkpoint = kind.name();
        }
        timing.finish(
            if committed.is_ok() {
                "committed"
            } else {
                "rollback"
            },
            committed.is_ok(),
        );
        committed
    }
    fn failed(&self, phase: &'static str, error: &crate::error::Error) {
        self.notify.failed(error);
        tracing::error!(name: "writer.failed",
             phase = phase,
             code = error.code(),
             error = error as &(dyn std::error::Error + 'static),
             sqlite_code = super::super::store::sqlite_code(error),
             decision = "shutdown");
        self.core.shutdown().store(true, Ordering::Relaxed);
        self.notify.all();
    }
    fn apply(&mut self, first: Job, timing: &mut Batch) -> Vec<Completion> {
        let began = Instant::now();
        let mut replies = vec![first.run(&mut self.core, timing)];
        while self.core.store().healthy().is_ok()
            && replies.len() < COMMIT_JOB_LIMIT
            && began.elapsed() < COMMIT_TIME_BUDGET
        {
            match self.receiver.try_recv() {
                Ok(job) => replies.push(job.run(&mut self.core, timing)),
                Err(_) => break,
            }
        }
        replies
    }
    #[expect(
        clippy::expect_used,
        reason = "A successful committed result requires readiness to have succeeded first."
    )]
    fn commit(&mut self, timing: &mut Batch, apply: Timer) -> Result<()> {
        let readiness = self
            .core
            .store()
            .settle_joins()
            .and_then(|()| self.observation.prepare(self.core.store()));
        timing.apply_ns = apply.elapsed_ns();
        let commit_start = Timer::start();
        let committed = readiness
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|_| self.core.store().commit());
        timing.commit_ns = commit_start.elapsed_ns();
        if let Err(error) = &committed {
            self.core.store().rollback_after(error);
        }
        if committed.is_ok() {
            self.observation_sequence += 1;
            // These counters describe the committed batch before its wakes drain.
            self.observation.publish(
                &self.core,
                self.observation_sequence,
                readiness.expect("commit requires readiness"),
            );
        }
        committed
    }
    fn publish_wakes(&mut self, committed: &Result<()>) {
        let crate::runtime::store::WakeChanges {
            agents: changed,
            all: overflow,
            deadlines,
            observed:
                crate::runtime::store::Observed {
                    agents: observed,
                    all: observed_all,
                    tree,
                },
        } = self.core.store().changes();
        let dirty = overflow || !changed.is_empty();
        if committed.is_ok() {
            // Viewers learn of durable changes without waking any worker.
            let live = &self.notify.live;
            if dirty || tree {
                live.tree_changed();
            }
            live.durable_changed(
                changed.iter().chain(&observed).map(String::as_str),
                overflow || observed_all,
            );
            if deadlines {
                self.notify.deadlines.notify_one();
            }
            for id in changed {
                self.notify.agent(&id);
            }
            if overflow {
                self.notify.all();
            }
            if dirty {
                self.notify.supervisor.notify_one();
            }
            if self.core.shutdown().load(Ordering::Relaxed) {
                self.notify.all();
            }
        } else {
            self.core.shutdown().store(true, Ordering::Relaxed);
            self.notify.all();
        }
    }
    /// At most one checkpoint per batch: the idle one, when due, subsumes the
    /// others.
    fn checkpoint_due(&self) -> Option<Checkpoint> {
        let idle = self.checkpoint_at.elapsed() >= CHECKPOINT_INTERVAL;
        #[cfg(test)]
        let idle = idle || self.core.store().test_hooks().checkpoint_pending();
        if self.receiver.is_empty() && idle {
            return Some(Checkpoint::Idle);
        }
        #[cfg(test)]
        if self.core.store().test_hooks().deferred_checkpoint_pending() {
            return Some(Checkpoint::Deferred);
        }
        let frames = wal::frames();
        match &self.checkpointer {
            // A long checkpoint stall lets the WAL grow; bound it on the writer.
            Some(_) if frames >= wal::FALLBACK => Some(Checkpoint::Deferred),
            Some(_) if frames >= wal::RESTART => Some(Checkpoint::Restart),
            Some(_) if frames >= wal::THRESHOLD => Some(Checkpoint::Background),
            None if frames >= wal::THRESHOLD => Some(Checkpoint::Deferred),
            _ => None,
        }
    }
    fn checkpoint(&mut self, kind: Checkpoint) {
        let result = match (kind, &self.checkpointer) {
            (Checkpoint::Background, Some(checkpointer)) => {
                checkpointer.request();
                return;
            }
            (Checkpoint::Restart, Some(checkpointer)) => {
                // Only a complete pass lets the next write restart the WAL.
                if checkpointer.drain(wal::RESTART_WAIT) {
                    wal::checkpointed(true);
                }
                return;
            }
            (Checkpoint::Idle, checkpointer) => {
                let _pass = checkpointer.as_ref().map(Checkpointer::exclusive);
                self.core.store().checkpoint()
            }
            // The writer's own checkpoint waits for a background pass, then keeps
            // the checkpointer out; it appends nothing meanwhile, so the WAL can
            // restart.
            (_, checkpointer) => {
                let _pass = checkpointer.as_ref().map(Checkpointer::exclusive);
                self.core.store().checkpoint_passive()
            }
        };
        match result {
            Ok(complete) => wal::checkpointed(complete),
            Err(error) => {
                tracing::warn!(name: "writer.checkpoint_failed",
                     code = error.code(),
                     error = &error as &(dyn std::error::Error + 'static),
                     sqlite_code = super::super::store::sqlite_code(&error),
                     decision = "defer");
            }
        }
        if kind == Checkpoint::Idle {
            self.checkpoint_at = Instant::now();
        }
    }
}
/// Every checkpoint runs after the batch's replies.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Checkpoint {
    /// Ask the checkpointer for a pass at SQLite's automatic threshold.
    Background,
    /// Wait briefly for a complete background pass, so that the WAL can restart.
    Restart,
    /// The writer's own PASSIVE: without a checkpointer at the threshold, or with
    /// one when the WAL reaches the fallback size.
    Deferred,
    /// PASSIVE then TRUNCATE, at most once per interval, only with an empty queue.
    Idle,
}
impl Checkpoint {
    fn name(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Restart => "restart",
            Self::Deferred => "deferred",
            Self::Idle => "idle",
        }
    }
}
